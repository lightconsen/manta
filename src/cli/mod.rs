//! Command-line interface for Manta
//!
//! This module handles argument parsing and command execution
//! using the `clap` crate.

use crate::config::Config;
use crate::core::models::{CreateEntityRequest, Status, UpdateEntityRequest};
use crate::core::Engine;
use crate::error::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, info, instrument, warn};

// Subcommand modules
mod admin;
mod agent;
mod channel;
mod chat;
mod cron;
mod daemon;
mod entity;
mod mcp;
mod plugin;
mod skill;
mod team;

pub use admin::AdminCommands;
pub use agent::AgentCommands;
pub use channel::ChannelCommands;
pub use cron::CronCommands;
pub use entity::EntityCommands;
pub use mcp::McpCommands;
pub use plugin::PluginCommands;
pub use skill::SkillCommands;
pub use team::TeamCommands;

/// Manta - Your AI assistant
#[derive(Debug, Parser)]
#[command(name = "manta")]
#[command(about = "Manta - Your AI assistant")]
#[command(version)]
pub struct Cli {
    /// Configuration file path
    #[arg(short, long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Log level override (trace, debug, info, warn, error)
    #[arg(short, long, global = true)]
    pub log_level: Option<String>,

    /// Subcommand to execute
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Entity management commands
    Entity {
        /// Entity subcommand
        #[command(subcommand)]
        command: EntityCommands,
    },
    /// Show configuration
    Config {
        /// Output format
        #[arg(short, long, value_enum, default_value = "toml")]
        format: ConfigFormat,
    },
    /// Health check
    Health,
    /// Chat with the AI assistant
    Chat {
        /// Use a specific conversation ID (for resuming conversations)
        #[arg(short, long)]
        conversation: Option<String>,
        /// Single message mode (non-interactive)
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Start web terminal interface
    Web {
        /// Port to listen on
        #[arg(short, long, default_value = "18081")]
        port: u16,
    },
    /// Run as an assistant process (internal use)
    AssistantRun {
        /// Configuration file path
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Admin commands for Gateway management (provider switching, status, etc.)
    Admin {
        /// Admin subcommand
        #[command(subcommand)]
        command: AdminCommands,
    },
    /// Cron job management
    Cron {
        /// Cron subcommand
        #[command(subcommand)]
        command: CronCommands,
    },
    /// Skill management commands
    Skill {
        /// Skill subcommand
        #[command(subcommand)]
        command: SkillCommands,
    },
    /// Agent personality management (OpenClaw-style memory files)
    Agent {
        /// Agent subcommand
        #[command(subcommand)]
        command: AgentCommands,
    },
    /// Agent team management (create teams, assign roles, define hierarchies)
    Team {
        /// Team subcommand
        #[command(subcommand)]
        command: TeamCommands,
    },
    /// Channel management (Telegram, Discord, Slack)
    Channel {
        /// Channel subcommand
        #[command(subcommand)]
        command: ChannelCommands,
    },
    /// Plugin management for WASM channel extensions
    #[command(name = "plugin")]
    Plugin {
        /// Plugin subcommand
        #[command(subcommand)]
        command: PluginCommands,
    },
    /// Start the Manta daemon (background server)
    Start {
        /// Host to bind to
        #[arg(short, long, default_value = "127.0.0.1")]
        host: String,
        /// API port to listen on
        #[arg(short, long, default_value = "18080")]
        port: u16,
        /// Web terminal port
        #[arg(short = 'w', long, default_value = "18081")]
        web_port: u16,
        /// Run in foreground (don't detach)
        #[arg(long)]
        foreground: bool,
    },
    /// Stop the Manta daemon
    Stop {
        /// Force kill if graceful shutdown fails
        #[arg(short, long)]
        force: bool,
    },
    /// Check daemon status
    Status,
    /// Show and tail daemon logs
    Logs {
        /// Number of lines to show (default: 50)
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
        /// Follow/tail the logs (like tail -f)
        #[arg(short, long)]
        follow: bool,
    },
    /// MCP (Model Context Protocol) management
    Mcp {
        /// MCP subcommand
        #[command(subcommand)]
        command: McpCommands,
    },
}

// AgentCommands is defined in agent.rs and re-exported here
// PluginCommands is defined in plugin.rs and re-exported here

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ChannelType {
    /// Telegram bot
    Telegram,
    /// Discord bot
    Discord,
    /// Slack bot
    Slack,
    /// WhatsApp bot
    Whatsapp,
    /// QQ bot
    Qq,
    /// Feishu/Lark bot
    Feishu,
    /// Custom WebSocket endpoint
    Websocket,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Json,
    Yaml,
    Table,
    Plain,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ConfigFormat {
    Toml,
    Json,
    Yaml,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum StorageType {
    /// SQLite storage (default, embedded)
    Sqlite,
    /// PostgreSQL storage (requires external database)
    Postgres,
    /// Redis storage (for caching and pub/sub)
    Redis,
}

fn init_logging(log_level: Option<&str>) {
    let level = log_level.unwrap_or("info");
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(format!("manta={},hyper=warn,reqwest=warn", level))
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .compact()
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set logger");
}

impl Cli {
    /// Run the CLI
    pub async fn run() -> Result<()> {
        let cli = Cli::parse();

        // Initialize logging
        init_logging(cli.log_level.as_deref());

        // Load configuration
        let config = if let Some(config_path) = &cli.config {
            Config::load_with_file(Some(config_path))?
        } else {
            Config::load()?
        };

        // Execute command
        cli.execute(&config).await
    }

    /// Execute the CLI command
    pub async fn execute(&self, config: &Config) -> Result<()> {
        match &self.command {
            Commands::Entity { command } => entity::run_entity_command(command).await,
            Commands::Config { format } => daemon::show_config(format).await,
            Commands::Health => daemon::run_health_check(config).await,
            Commands::Chat { conversation, message } => {
                chat::run_chat(config, conversation.clone(), message.clone()).await
            }
            Commands::Web { port } => chat::run_web(config, *port).await,
            Commands::AssistantRun { config: config_path } => {
                daemon::run_assistant_process(config_path).await
            }
            Commands::Admin { command } => admin::run_admin_command(command).await,
            Commands::Cron { command } => cron::run_cron_command(command).await,
            Commands::Skill { command } => skill::run_skill_command(command).await,
            Commands::Agent { command } => agent::run_agent_command(command).await,
            Commands::Team { command } => team::run_team_command(command).await,
            Commands::Channel { command } => channel::run_channel_command(command).await,
            Commands::Plugin { command } => plugin::run_plugin_command(command).await,
            Commands::Start {
                host,
                port,
                web_port,
                foreground,
            } => daemon::run_start_daemon(host, *port, *web_port, *foreground, config).await,
            Commands::Stop { force } => daemon::run_stop_daemon(*force).await,
            Commands::Status => daemon::run_daemon_status().await,
            Commands::Logs { lines, follow } => daemon::run_logs(*lines, *follow).await,
            Commands::Mcp { command } => mcp::run_mcp_command(command).await,
        }
    }
}
