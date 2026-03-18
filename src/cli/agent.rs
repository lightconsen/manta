//! Agent personality management commands for Manta

use clap::Subcommand;
use crate::error::Result;
use std::path::PathBuf;

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
    match command {
        AgentCommands::List { all } => {
            println!("Listing agents (all={})", all);
        }
        AgentCommands::Show { name } => {
            println!("Showing agent: {:?}", name);
        }
        AgentCommands::Create { name, description, copy_from } => {
            println!("Creating agent {}: {:?}, copy_from: {:?}", name, description, copy_from);
        }
        AgentCommands::Edit { name } => {
            println!("Editing agent: {}", name);
        }
        AgentCommands::Delete { name, force } => {
            println!("Deleting agent {} (force={})", name, force);
        }
        AgentCommands::Switch { name } => {
            println!("Switching to agent: {}", name);
        }
        AgentCommands::Memory { name, clear } => {
            println!("Agent memory: {:?}, clear={}", name, clear);
        }
        AgentCommands::Import { path, name } => {
            println!("Importing agent from {:?} as {:?}", path, name);
        }
        AgentCommands::Export { name, output } => {
            println!("Exporting agent {} to {:?}", name, output);
        }
    }
    Ok(())
}
