//! Plugin management commands for Manta

use clap::Subcommand;
use crate::error::Result;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum PluginCommands {
    /// List all plugins
    List {
        /// Show loaded plugins only
        #[arg(short, long)]
        loaded: bool,
        /// Show verbose information
        #[arg(short, long)]
        verbose: bool,
    },
    /// Install a plugin from a file
    Install {
        /// Path to plugin file (.wasm)
        path: PathBuf,
        /// Plugin name (optional, defaults to file name)
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Uninstall a plugin
    Uninstall {
        /// Plugin name
        name: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Enable a plugin
    Enable {
        /// Plugin name
        name: String,
    },
    /// Disable a plugin
    Disable {
        /// Plugin name
        name: String,
    },
    /// Show plugin information
    Info {
        /// Plugin name
        name: String,
    },
    /// Reload plugins (hot-reload)
    Reload,
}

/// Run plugin commands
pub async fn run_plugin_command(command: &PluginCommands) -> Result<()> {
    match command {
        PluginCommands::List { loaded, verbose } => {
            println!("🔌 WASM Channel Plugins");
            println!("======================");
            println!("Loaded: {}, Verbose: {}", loaded, verbose);
        }
        PluginCommands::Install { path, name } => {
            println!("Installing plugin from {:?} as {:?}", path, name);
        }
        PluginCommands::Uninstall { name, force } => {
            println!("Uninstalling plugin {} (force={})", name, force);
        }
        PluginCommands::Enable { name } => {
            println!("Enabling plugin: {}", name);
        }
        PluginCommands::Disable { name } => {
            println!("Disabling plugin: {}", name);
        }
        PluginCommands::Info { name } => {
            println!("Plugin info: {}", name);
        }
        PluginCommands::Reload => {
            println!("Reloading plugins...");
        }
    }
    Ok(())
}
