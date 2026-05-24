//! Provider management commands for Manta
//!
//! Top-level CLI for listing, enabling, disabling, and switching model providers.

use crate::error::{MantaError, Result};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum ProviderCommands {
    /// List available LLM providers
    List,
    /// Show provider health status
    Health {
        /// Provider ID
        id: String,
    },
    /// Enable a provider
    Enable {
        /// Provider ID
        id: String,
    },
    /// Disable a provider
    Disable {
        /// Provider ID
        id: String,
    },
    /// Switch the default model alias
    Switch {
        /// Model alias (fast, smart, default)
        alias: String,
    },
    /// Show current default model
    Default,
    /// Show provider usage statistics
    Usage {
        /// Provider ID (omit for all providers)
        id: Option<String>,
    },
}

/// Run provider commands
pub async fn run_provider_command(command: &ProviderCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        ProviderCommands::List => {
            let url = format!("{}/api/v1/providers", DAEMON_URL);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    if let Some(providers) = body.get("providers").and_then(|p| p.as_array()) {
                        println!("Providers:");
                        println!("{:<20} {:<10} {:<10} {}", "ID", "Enabled", "Healthy", "Name");
                        println!("{}", "-".repeat(60));
                        for p in providers {
                            println!(
                                "{:<20} {:<10} {:<10} {}",
                                p.get("id").and_then(|c| c.as_str()).unwrap_or("-"),
                                if p.get("enabled").and_then(|c| c.as_bool()).unwrap_or(false) {
                                    "yes"
                                } else {
                                    "no"
                                },
                                if p.get("healthy").and_then(|c| c.as_bool()).unwrap_or(false) {
                                    "yes"
                                } else {
                                    "no"
                                },
                                p.get("name").and_then(|c| c.as_str()).unwrap_or("-"),
                            );
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
        ProviderCommands::Health { id } => {
            let url = format!("{}/api/v1/providers/{}/health", DAEMON_URL, id);
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
            Ok(())
        }
        ProviderCommands::Enable { id } => {
            let url = format!("{}/api/v1/providers/{}/enable", DAEMON_URL, id);
            match client.post(&url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("✅ Enabled provider {}", id);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to enable: {}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        ProviderCommands::Disable { id } => {
            let url = format!("{}/api/v1/providers/{}/disable", DAEMON_URL, id);
            match client.post(&url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("✅ Disabled provider {}", id);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to disable: {}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        ProviderCommands::Switch { alias } => {
            let url = format!("{}/api/v1/providers/switch", DAEMON_URL);
            let body = serde_json::json!({ "model": alias });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("✅ Switched default model to {}", alias);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to switch: {}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        ProviderCommands::Default => {
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
            Ok(())
        }
        ProviderCommands::Usage { id } => {
            let url = if let Some(ref provider_id) = id {
                format!("{}/api/v1/providers/usage/{}", DAEMON_URL, provider_id)
            } else {
                format!("{}/api/v1/providers/usage", DAEMON_URL)
            };
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    // Try to parse as formatted usage snapshots
                    if let Ok(snapshots) = serde_json::from_value::<
                        Vec<crate::model_router::ProviderUsageSnapshot>,
                    >(body.clone())
                    {
                        if id.is_some() {
                            for snapshot in &snapshots {
                                println!(
                                    "{}",
                                    crate::model_router::format_provider_snapshot(snapshot)
                                );
                            }
                        } else {
                            println!("{}", crate::model_router::format_usage_report(&snapshots));
                        }
                    } else {
                        // Fallback to pretty-printed JSON
                        println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
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
