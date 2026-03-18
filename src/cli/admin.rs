//! Admin commands for Gateway management

use crate::error::Result;
use clap::Subcommand;

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
    match command {
        AdminCommands::Status => {
            println!("Gateway status...");
        }
        AdminCommands::Providers => {
            println!("Listing providers...");
        }
        AdminCommands::Models => {
            println!("Listing models...");
        }
        AdminCommands::Default => {
            println!("Showing default model...");
        }
        AdminCommands::Switch { model } => {
            println!("Switching to model: {}", model);
        }
        AdminCommands::Enable { provider } => {
            println!("Enabling provider: {}", provider);
        }
        AdminCommands::Disable { provider } => {
            println!("Disabling provider: {}", provider);
        }
        AdminCommands::Health { provider } => {
            println!("Checking health for: {}", provider);
        }
        AdminCommands::Fallback { alias } => {
            println!("Showing fallback for: {}", alias);
        }
        AdminCommands::Agents => {
            println!("Listing agents...");
        }
        AdminCommands::Send { session_id, message, provider, model } => {
            println!("Sending to {}: {}", session_id, message);
            if let Some(p) = provider {
                println!("  Provider: {}", p);
            }
            if let Some(m) = model {
                println!("  Model: {}", m);
            }
        }
    }
    Ok(())
}
