//! Skill management commands for Manta

use clap::Subcommand;
use crate::cli::OutputFormat;
use crate::error::Result;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum SkillCommands {
    /// List all available skills
    List {
        /// Show all skills including ineligible ones
        #[arg(short, long)]
        all: bool,
        /// Output format
        #[arg(short, long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Show detailed information about a skill
    Info {
        /// Skill name
        name: String,
    },
    /// Install a skill from a directory or git repo
    Install {
        /// Path to skill directory or git URL
        source: String,
        /// Skill name (optional, defaults to directory name)
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Uninstall a skill
    Uninstall {
        /// Skill name
        name: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Enable a skill
    Enable {
        /// Skill name
        name: String,
    },
    /// Disable a skill
    Disable {
        /// Skill name
        name: String,
    },
    /// Install dependencies for a skill
    Setup {
        /// Skill name (if not provided, sets up all eligible skills)
        name: Option<String>,
    },
    /// Create a new skill template
    Init {
        /// Skill name
        name: String,
        /// Target directory (defaults to ./<name>-skill)
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Template to use
        #[arg(short, long, default_value = "basic")]
        template: String,
    },
}

/// Run skill commands
pub async fn run_skill_command(command: &SkillCommands) -> Result<()> {
    match command {
        SkillCommands::List { all, format } => {
            println!("Listing skills (all={}, format={:?})", all, format);
        }
        SkillCommands::Info { name } => {
            println!("Skill info: {}", name);
        }
        SkillCommands::Install { source, name } => {
            println!("Installing skill from {} as {:?}", source, name);
        }
        SkillCommands::Uninstall { name, force } => {
            println!("Uninstalling skill {} (force={})", name, force);
        }
        SkillCommands::Enable { name } => {
            println!("Enabling skill: {}", name);
        }
        SkillCommands::Disable { name } => {
            println!("Disabling skill: {}", name);
        }
        SkillCommands::Setup { name } => {
            println!("Setting up skill: {:?}", name);
        }
        SkillCommands::Init { name, path, template } => {
            println!("Creating skill {} at {:?} using template {}", name, path, template);
        }
    }
    Ok(())
}
