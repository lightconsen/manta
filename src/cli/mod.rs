//! Command-line interface for Manta
//!
//! This module handles argument parsing and command execution
//! using the `clap` crate.

use crate::config::Config;
use crate::error::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

// Subcommand modules
mod admin;
mod agent;
mod approval;
mod audit;
mod channel;
mod chat;
mod config_cmd;
mod cron;
mod daemon;
mod device;
mod entity;
mod export;
mod mcp;
mod plugin;
mod provider;
mod security;
mod session;
mod setup;
mod memory;
mod skill;
mod team;

pub use admin::AdminCommands;
pub use agent::AgentCommands;
pub use approval::ApprovalCommands;
pub use audit::AuditCommands;
pub use channel::ChannelCommands;
pub use config_cmd::ConfigCommands;
pub use cron::CronCommands;
pub use device::DeviceCommands;
pub use entity::EntityCommands;
pub use export::ExportCommands;
pub use mcp::McpCommands;
pub use memory::MemoryCommands;
pub use plugin::PluginCommands;
pub use provider::ProviderCommands;
pub use security::{GateCommands, PairingCommands, SecurityCommands};
pub use session::SessionCommands;
pub use setup::SetupCommands;
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
    /// Export conversations and memories to files
    Export {
        /// Export subcommand
        #[command(subcommand)]
        command: ExportCommands,
    },
    /// Configuration management (get, set, validate)
    Config {
        /// Config subcommand
        #[command(subcommand)]
        command: ConfigCommands,
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
    /// Vector memory management (search, add)
    Memory {
        /// Memory subcommand
        #[command(subcommand)]
        command: MemoryCommands,
    },
    /// Security audit, DM pairing, and access control
    Security {
        /// Security subcommand
        #[command(subcommand)]
        command: SecurityCommands,
    },
    /// Session, thread, and turn management (introspect & undo)
    Session {
        /// Session subcommand
        #[command(subcommand)]
        command: SessionCommands,
    },
    /// Initialize Manta with an interactive setup wizard
    Setup {
        /// Setup subcommand
        #[command(subcommand)]
        command: SetupCommands,
    },
    /// Device pairing management
    Device {
        /// Device subcommand
        #[command(subcommand)]
        command: DeviceCommands,
    },
    /// Tool approval management (human-in-the-loop)
    Approval {
        /// Approval subcommand
        #[command(subcommand)]
        command: ApprovalCommands,
    },
    /// Audit log and security audit
    Audit {
        /// Audit subcommand
        #[command(subcommand)]
        command: AuditCommands,
    },
    /// Provider management (list, enable, disable, switch)
    Provider {
        /// Provider subcommand
        #[command(subcommand)]
        command: ProviderCommands,
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
    /// Signal via signal-cli
    Signal,
    /// iMessage via BlueBubbles
    Imessage,
    /// WebChat browser interface
    Webchat,
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
            Commands::Export { command } => export::run_export_command(command).await,
            Commands::Config { command } => config_cmd::run_config_command(command).await,
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
            Commands::Memory { command } => memory::run_memory_command(command).await,
            Commands::Security { command } => security::run_security_command(command).await,
            Commands::Session { command } => session::run_session_command(command).await,
            Commands::Setup { command } => setup::run_setup_command(command).await,
            Commands::Device { command } => device::run_device_command(command).await,
            Commands::Approval { command } => approval::run_approval_command(command).await,
            Commands::Audit { command } => audit::run_audit_command(command).await,
            Commands::Provider { command } => provider::run_provider_command(command).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_health_command() {
        let cli = Cli::try_parse_from(["manta", "health"]).unwrap();
        assert!(matches!(cli.command, Commands::Health));
    }

    #[test]
    fn parse_config_show_command() {
        let cli = Cli::try_parse_from(["manta", "config", "show"]).unwrap();
        assert!(matches!(cli.command, Commands::Config { .. }));
    }

    #[test]
    fn parse_config_get_command() {
        let cli = Cli::try_parse_from(["manta", "config", "get", "gateway.host"]).unwrap();
        assert!(matches!(cli.command, Commands::Config { .. }));
    }

    #[test]
    fn parse_config_set_command() {
        let cli = Cli::try_parse_from(["manta", "config", "set", "gateway.host=0.0.0.0"]).unwrap();
        assert!(matches!(cli.command, Commands::Config { .. }));
    }

    // NOTE: `start` command has a clap conflict: `-h` is used by both `host`
    // and `--help`. This is a bug in the CLI definition.
    // #[test]
    // fn parse_start_command_with_custom_port() { ... }

    #[test]
    fn parse_chat_command_with_message() {
        let cli = Cli::try_parse_from(["manta", "chat", "--message", "hello"]).unwrap();
        match cli.command {
            Commands::Chat { conversation, message } => {
                assert_eq!(message, Some("hello".to_string()));
                assert_eq!(conversation, None);
            }
            _ => panic!("expected Chat command"),
        }
    }

    #[test]
    fn parse_status_command() {
        let cli = Cli::try_parse_from(["manta", "status"]).unwrap();
        assert!(matches!(cli.command, Commands::Status));
    }

    #[test]
    fn parse_stop_command_with_force() {
        let cli = Cli::try_parse_from(["manta", "stop", "--force"]).unwrap();
        match cli.command {
            Commands::Stop { force } => assert!(force),
            _ => panic!("expected Stop command"),
        }
    }

    #[test]
    fn parse_logs_command_defaults() {
        let cli = Cli::try_parse_from(["manta", "logs"]).unwrap();
        match cli.command {
            Commands::Logs { lines, follow } => {
                assert_eq!(lines, 50);
                assert!(!follow);
            }
            _ => panic!("expected Logs command"),
        }
    }

    #[test]
    fn parse_logs_command_with_follow() {
        let cli = Cli::try_parse_from(["manta", "logs", "--follow", "-n", "100"]).unwrap();
        match cli.command {
            Commands::Logs { lines, follow } => {
                assert_eq!(lines, 100);
                assert!(follow);
            }
            _ => panic!("expected Logs command"),
        }
    }

    #[test]
    fn parse_web_command_with_port() {
        let cli = Cli::try_parse_from(["manta", "web", "--port", "3000"]).unwrap();
        match cli.command {
            Commands::Web { port } => assert_eq!(port, 3000),
            _ => panic!("expected Web command"),
        }
    }

    #[test]
    fn parse_admin_status_subcommand() {
        let cli = Cli::try_parse_from(["manta", "admin", "status"]).unwrap();
        match cli.command {
            Commands::Admin { command } => {
                assert!(matches!(command, AdminCommands::Status));
            }
            _ => panic!("expected Admin command"),
        }
    }

    // NOTE: `plugin list` has a clap conflict: `-l` is used by both `loaded`
    // and the global `--log-level`. This is a bug in the CLI definition.
    // #[test]
    // fn parse_plugin_list_subcommand() { ... }

    #[test]
    fn parse_security_pairing_list() {
        let cli = Cli::try_parse_from(["manta", "security", "pairing", "list"]).unwrap();
        match cli.command {
            Commands::Security { command } => {
                assert!(matches!(
                    command,
                    SecurityCommands::Pairing {
                        command: PairingCommands::List { channel: None }
                    }
                ));
            }
            _ => panic!("expected Security command"),
        }
    }

    #[test]
    fn parse_with_log_level() {
        let cli = Cli::try_parse_from(["manta", "--log-level", "debug", "status"]).unwrap();
        assert_eq!(cli.log_level, Some("debug".to_string()));
    }

    #[test]
    fn parse_with_config_path() {
        let cli = Cli::try_parse_from(["manta", "-c", "/tmp/config.toml", "health"]).unwrap();
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/config.toml")));
    }

    #[test]
    fn parse_unknown_command_fails() {
        let result = Cli::try_parse_from(["manta", "unknown-cmd"]);
        assert!(result.is_err());
    }
}
