//! Admin commands for Gateway management

use crate::error::{MantaError, Result};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum AdminCommands {
    /// Show Gateway status
    Status,
    /// List available LLM providers
    Providers,
    /// List available model aliases
    Models,
    /// Show current default model
    Default,
    /// Switch the default model alias
    Switch {
        /// Model alias to switch to (fast, smart, default)
        model: String,
    },
    /// Enable a provider
    Enable {
        /// Provider name
        provider: String,
    },
    /// Disable a provider
    Disable {
        /// Provider name
        provider: String,
    },
    /// Check provider health
    Health {
        /// Provider name
        provider: String,
    },
    /// Show fallback chain for an alias
    Fallback {
        /// Model alias
        alias: String,
    },
    /// List all agents
    Agents,
    /// Send a message to a session (with optional provider override)
    Send {
        /// Session ID
        session_id: String,
        /// Message content
        message: String,
        /// Optional provider override
        #[arg(short, long)]
        provider: Option<String>,
        /// Optional model alias override
        #[arg(short, long)]
        model: Option<String>,
    },
}

/// Run admin commands
pub async fn run_admin_command(command: &AdminCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        AdminCommands::Status => {
            let url = format!("{}/api/v1/status", DAEMON_URL);
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
        AdminCommands::Providers => {
            let url = format!("{}/api/v1/providers", DAEMON_URL);
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
        AdminCommands::Models => {
            let url = format!("{}/api/v1/models", DAEMON_URL);
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
        AdminCommands::Default => {
            let url = format!("{}/api/v1/models/default", DAEMON_URL);
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
        AdminCommands::Switch { model } => {
            let url = format!("{}/api/v1/providers/switch", DAEMON_URL);
            let body = serde_json::json!({ "model": model });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Switched to model alias '{}'", model);
                        println!("{}", text);
                    } else {
                        eprintln!("Failed to switch model ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        AdminCommands::Enable { provider } => {
            let url = format!("{}/api/v1/providers/{}/enable", DAEMON_URL, provider);
            match client.post(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Provider '{}' enabled", provider);
                    } else {
                        eprintln!("Failed to enable provider ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        AdminCommands::Disable { provider } => {
            let url = format!("{}/api/v1/providers/{}/disable", DAEMON_URL, provider);
            match client.post(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Provider '{}' disabled", provider);
                    } else {
                        eprintln!("Failed to disable provider ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        AdminCommands::Health { provider } => {
            let url = format!("{}/api/v1/providers/{}/health", DAEMON_URL, provider);
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
        AdminCommands::Fallback { alias } => {
            let url = format!("{}/api/v1/providers/fallback/{}", DAEMON_URL, alias);
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
        AdminCommands::Agents => {
            let url = format!("{}/api/v1/agents", DAEMON_URL);
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
        AdminCommands::Send { session_id, message, provider, model } => {
            let url = format!("{}/api/v1/sessions/{}/messages", DAEMON_URL, session_id);
            let body = serde_json::json!({
                "content": message,
                "provider": provider,
                "model": model,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("{}", text);
                    } else {
                        eprintln!("Failed to send message ({}): {}", status, text);
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
