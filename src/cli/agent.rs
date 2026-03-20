//! Agent personality management commands for Manta

use crate::error::{MantaError, Result};
use clap::Subcommand;
use std::path::PathBuf;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum AgentCommands {
    /// List all agent personalities
    List {
        /// Show all agents including system defaults
        #[arg(short, long)]
        all: bool,
    },
    /// Show current agent configuration
    Show {
        /// Agent name (defaults to current agent)
        name: Option<String>,
    },
    /// Create a new agent personality
    Create {
        /// Agent name
        name: String,
        /// Description of the agent's role
        #[arg(short, long)]
        description: Option<String>,
        /// Copy from existing agent
        #[arg(short, long)]
        copy_from: Option<String>,
    },
    /// Edit agent configuration
    Edit {
        /// Agent name
        name: String,
    },
    /// Delete an agent personality
    Delete {
        /// Agent name
        name: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Switch to a different agent
    Switch {
        /// Agent name
        name: String,
    },
    /// Show agent memory/state
    Memory {
        /// Agent name
        name: Option<String>,
        /// Clear memory
        #[arg(long)]
        clear: bool,
    },
    /// Import an agent from a file
    Import {
        /// Path to agent configuration file
        path: PathBuf,
        /// Agent name (optional, defaults to file name)
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Export an agent to a file
    Export {
        /// Agent name
        name: String,
        /// Output path
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

/// Run agent commands
pub async fn run_agent_command(command: &AgentCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        AgentCommands::List { all: _ } => {
            let url = format!("{}/api/v1/agents", DAEMON_URL);
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
        AgentCommands::Show { name } => {
            let id = name.as_deref().unwrap_or("default");
            let url = format!("{}/api/v1/agents/{}", DAEMON_URL, id);
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
        AgentCommands::Create { name, description, copy_from } => {
            let url = format!("{}/api/v1/agents", DAEMON_URL);
            let body = serde_json::json!({
                "name": name,
                "description": description,
                "copy_from": copy_from,
                "system_prompt": description.clone().unwrap_or_default(),
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Agent '{}' created successfully", name);
                        println!("{}", text);
                    } else {
                        eprintln!("Failed to create agent ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        AgentCommands::Edit { name } => {
            // For edit, show current config and prompt user to use the API
            let url = format!("{}/api/v1/agents/{}", DAEMON_URL, name);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    println!("Current config for agent '{}':", name);
                    println!("{}", body);
                    println!("\nUse PATCH {}/api/v1/agents/{} to update", DAEMON_URL, name);
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        AgentCommands::Delete { name, force } => {
            if !force {
                println!("Delete agent '{}'? Use --force to confirm.", name);
                return Ok(());
            }
            let url = format!("{}/api/v1/agents/{}", DAEMON_URL, name);
            match client.delete(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        println!("Agent '{}' deleted", name);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to delete agent ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        AgentCommands::Switch { name } => {
            let url = format!("{}/api/v1/agents/default", DAEMON_URL);
            let body = serde_json::json!({ "agent_id": name });
            match client.patch(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        println!("Switched to agent '{}'", name);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to switch agent ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        AgentCommands::Memory { name, clear } => {
            let id = name.as_deref().unwrap_or("default");
            if *clear {
                let url = format!("{}/api/v1/agents/{}/memory", DAEMON_URL, id);
                match client.delete(&url).send().await {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            println!("Memory cleared for agent '{}'", id);
                        } else {
                            let text = resp.text().await.unwrap_or_default();
                            eprintln!("Failed to clear memory: {}", text);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to reach daemon: {}", e);
                        return Err(MantaError::Internal(e.to_string()));
                    }
                }
            } else {
                let url = format!("{}/api/v1/agents/{}/memory", DAEMON_URL, id);
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
        AgentCommands::Import { path, name } => {
            let content = tokio::fs::read_to_string(path).await.map_err(|e| {
                MantaError::Internal(format!("Failed to read file: {}", e))
            })?;
            let mut body: serde_json::Value =
                serde_json::from_str(&content).unwrap_or(serde_json::json!({}));
            if let Some(n) = name {
                body["name"] = serde_json::Value::String(n.clone());
            }
            let url = format!("{}/api/v1/agents", DAEMON_URL);
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Agent imported successfully");
                        println!("{}", text);
                    } else {
                        eprintln!("Failed to import agent ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        AgentCommands::Export { name, output } => {
            let url = format!("{}/api/v1/agents/{}", DAEMON_URL, name);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    if let Some(path) = output {
                        tokio::fs::write(path, &body).await.map_err(|e| {
                            MantaError::Internal(format!("Failed to write file: {}", e))
                        })?;
                        println!("Agent '{}' exported to {:?}", name, path);
                    } else {
                        println!("{}", body);
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
