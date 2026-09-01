//! Admin commands for Gateway management

use clap::Subcommand;
use serde_json::json;

use crate::cli::ws;
use crate::error::Result;

#[derive(Debug, Subcommand)]
pub enum AdminCommands {
    /// Show Gateway status
    Status,
    /// List available LLM providers
    Providers,
    /// List available models
    Models,
    /// Show current default model
    Default,
    /// Switch the default model
    Switch {
        /// Concrete model ID to switch to
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
    /// Show fallback chain for a model
    Fallback {
        /// Model ID
        model_id: String,
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
        /// Optional model ID override
        #[arg(short, long)]
        model: Option<String>,
    },
}

/// Run admin commands (over WebSocket where possible).
pub async fn run_admin_command(command: &AdminCommands) -> Result<()> {
    match command {
        AdminCommands::Status => {
            let payload = ws::call("status.get", json!({})).await?;
            println!("{}", payload);
        }
        AdminCommands::Providers => {
            let payload = ws::call("providers.list", json!({})).await?;
            println!("{}", payload);
        }
        AdminCommands::Models => {
            let payload = ws::call("models.list", json!({})).await?;
            println!("{}", payload);
        }
        AdminCommands::Default => {
            let payload = ws::call("models.default", json!({})).await?;
            println!("{}", payload);
        }
        AdminCommands::Switch { model } => {
            match ws::call("providers.switch", json!({ "model": model })).await {
                Ok(payload) => {
                    println!("Switched to model '{}'", model);
                    println!("{}", payload);
                }
                Err(e) => {
                    eprintln!("Failed to switch model: {}", e);
                    return Err(e);
                }
            }
        }
        AdminCommands::Enable { provider } => {
            match ws::call("providers.enable", json!({ "id": provider })).await {
                Ok(_) => println!("Provider '{}' enabled", provider),
                Err(e) => {
                    eprintln!("Failed to enable provider: {}", e);
                    return Err(e);
                }
            }
        }
        AdminCommands::Disable { provider } => {
            match ws::call("providers.disable", json!({ "id": provider })).await {
                Ok(_) => println!("Provider '{}' disabled", provider),
                Err(e) => {
                    eprintln!("Failed to disable provider: {}", e);
                    return Err(e);
                }
            }
        }
        AdminCommands::Health { provider } => {
            let payload = ws::call("providers.health", json!({ "id": provider })).await?;
            println!("{}", payload);
        }
        AdminCommands::Fallback { model_id } => {
            let payload = ws::call("providers.fallback", json!({ "model_id": model_id })).await?;
            println!("{}", payload);
        }
        AdminCommands::Agents => {
            let payload = ws::call("agents.list", json!({})).await?;
            println!("{}", payload);
        }
        AdminCommands::Send {
            session_id,
            message,
            provider: _provider,
            model: _model,
        } => {
            // Send to an existing session over WS `chat.send`. The provider /
            // model overrides are not part of the WS surface, so they are
            // accepted for CLI compatibility but not forwarded.
            match ws::call("chat.send", json!({ "message": message, "session_id": session_id }))
                .await
            {
                Ok(payload) => {
                    println!("{}", serde_json::to_string_pretty(&payload).unwrap_or_default());
                }
                Err(e) => {
                    eprintln!("Failed to send message: {}", e);
                    return Err(e);
                }
            }
        }
    }
    Ok(())
}
