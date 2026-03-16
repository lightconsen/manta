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
use std::sync::Arc;
use tracing::{debug, info, instrument};

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
    /// Channel management (Telegram, Discord, Slack)
    Channel {
        /// Channel subcommand
        #[command(subcommand)]
        command: ChannelCommands,
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
}

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
        /// Skill description
        #[arg(short, long)]
        description: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum EntityCommands {
    /// Create a new entity
    Create {
        /// Entity name
        name: String,
        /// Optional description
        #[arg(short, long)]
        description: Option<String>,
        /// Tags to attach
        #[arg(short, long)]
        tags: Vec<String>,
    },
    /// Get an entity by ID
    Get {
        /// Entity ID
        id: String,
        /// Output format
        #[arg(short, long, value_enum, default_value = "json")]
        format: OutputFormat,
    },
    /// List all entities
    List {
        /// Filter by status
        #[arg(short, long, value_enum)]
        status: Option<StatusFilter>,
        /// Output format
        #[arg(short, long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Update an entity
    Update {
        /// Entity ID
        id: String,
        /// New name
        #[arg(short, long)]
        name: Option<String>,
        /// New description
        #[arg(short, long)]
        description: Option<String>,
        /// New status
        #[arg(short, long, value_enum)]
        status: Option<StatusFilter>,
    },
    /// Delete an entity
    Delete {
        /// Entity ID
        id: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum CronCommands {
    /// List all cron jobs
    List,
    /// Add a new cron job
    Add {
        /// Job name
        name: String,
        /// Cron schedule expression (e.g., "0 * * * *" for hourly)
        #[arg(short, long)]
        schedule: String,
        /// Command to execute
        #[arg(short, long)]
        command: String,
        /// Description
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Remove a cron job
    Remove {
        /// Job name
        name: String,
    },
    /// Enable a cron job
    Enable {
        /// Job name
        name: String,
    },
    /// Disable a cron job
    Disable {
        /// Job name
        name: String,
    },
    /// Run a job immediately (one-time execution)
    Run {
        /// Job name
        name: String,
    },
    /// Show cron job status and next run times
    Status,
}

#[derive(Debug, Subcommand)]
pub enum AgentCommands {
    /// Create a new agent personality
    Create {
        /// Agent name (used as directory name)
        name: String,
        /// Agent display name (defaults to name if not provided)
        #[arg(short, long)]
        display_name: Option<String>,
        /// Agent role/purpose
        #[arg(short, long)]
        role: Option<String>,
        /// Communication style (concise, detailed, friendly, professional)
        #[arg(short, long, default_value = "professional")]
        style: String,
        /// Initial system prompt
        #[arg(short, long)]
        prompt: Option<String>,
        /// Output format (markdown, yaml, json)
        #[arg(short, long, default_value = "markdown")]
        format: String,
    },
    /// Remove an agent personality
    Remove {
        /// Agent name
        name: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// List all agent personalities
    List {
        /// Show all details
        #[arg(short, long)]
        verbose: bool,
    },
    /// Set the active agent personality
    Set {
        /// Agent name (or "default" to reset)
        name: String,
    },
    /// Show current agent personality details
    Show,
    /// Edit an agent personality file
    Edit {
        /// Agent name
        name: String,
        /// Which file to edit (soul, identity, bootstrap, or all)
        #[arg(short, long, default_value = "all")]
        file: String,
    },
    /// Initialize workspace-level memory files (like OpenClaw)
    InitWorkspace {
        /// Force overwrite existing files
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ChannelCommands {
    /// Add/connect a channel
    Add {
        /// Channel type to add
        #[arg(value_enum)]
        channel: ChannelType,
        /// Bot token (can also use env var: TELEGRAM_BOT_TOKEN, DISCORD_BOT_TOKEN, etc.)
        #[arg(short, long)]
        token: Option<String>,
        /// Run in foreground (don't detach)
        #[arg(long)]
        foreground: bool,
    },
    /// Remove/disconnect a channel
    Remove {
        /// Channel type to remove
        #[arg(value_enum)]
        channel: ChannelType,
    },
    /// List configured channels
    List {
        /// Show all channels including disabled ones
        #[arg(short, long)]
        all: bool,
    },
    /// Check channel status
    Status {
        /// Channel type (if not provided, shows all channels)
        #[arg(value_enum)]
        channel: Option<ChannelType>,
    },
    /// Test channel configuration
    Test {
        /// Channel type to test
        #[arg(value_enum)]
        channel: ChannelType,
    },
}

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
    /// Lark/Feishu bot
    Lark,
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
pub enum StatusFilter {
    Pending,
    Active,
    Paused,
    Completed,
    Failed,
    Archived,
}

impl From<StatusFilter> for Status {
    fn from(filter: StatusFilter) -> Self {
        match filter {
            StatusFilter::Pending => Status::Pending,
            StatusFilter::Active => Status::Active,
            StatusFilter::Paused => Status::Paused,
            StatusFilter::Completed => Status::Completed,
            StatusFilter::Failed => Status::Failed,
            StatusFilter::Archived => Status::Archived,
        }
    }
}

impl Cli {
    /// Parse CLI arguments and execute the command
    pub async fn run() -> Result<()> {
        let cli = Self::parse();
        cli.execute().await
    }

    /// Execute the parsed command
    #[instrument(skip(self))]
    pub async fn execute(&self) -> Result<()> {
        debug!("Executing CLI command");

        // Load configuration
        let mut config = if let Some(ref path) = self.config {
            Config::load_with_file(Some(path))?
        } else {
            Config::load()?
        };

        // Override log level if specified
        if let Some(ref level) = self.log_level {
            config.logging.level = level.clone();
        }

        // Initialize logging
        crate::utils::logging::init_logging(&config)?;
        crate::utils::logging::setup_panic_handler();

        info!("Manta starting up");

        match &self.command {
            Commands::Entity { command } => self.run_entity_command(command).await,
            Commands::Config { format } => self.show_config(&config, *format),
            Commands::Health => self.run_health_check(&config).await,
            Commands::Chat { conversation, message } => {
                self.run_chat(&config, conversation.clone(), message.clone()).await
            }
            Commands::AssistantRun { config: assistant_config } => {
                self.run_assistant_process(assistant_config).await
            }
            Commands::Admin { command } => {
                self.run_admin_command(command).await
            }
            Commands::Web { port } => {
                self.run_web(&config, *port).await
            }
            Commands::Cron { command } => self.run_cron_command(command).await,
            Commands::Skill { command } => self.run_skill_command(command).await,
            Commands::Agent { command } => self.run_agent_command(command).await,
            Commands::Channel { command } => self.run_channel_command(command).await,
            Commands::Start { host, port, web_port, foreground } => {
                self.run_start_daemon(host, *port, *web_port, *foreground).await
            },
            Commands::Stop { force } => self.run_stop_daemon(*force).await,
            Commands::Status => self.run_daemon_status().await,
            Commands::Logs { lines, follow } => self.run_logs(*lines, *follow).await,
        }
    }

    async fn run_entity_command(&self, command: &EntityCommands) -> Result<()> {
        let engine = Engine::new();

        match command {
            EntityCommands::Create {
                name,
                description,
                tags,
            } => {
                let request = CreateEntityRequest {
                    name: name.clone(),
                    description: description.clone(),
                    tags: if tags.is_empty() {
                        None
                    } else {
                        Some(tags.clone())
                    },
                };

                let entity = engine.create_entity(request)?;
                println!("✅ Created entity:");
                println!("   ID:          {}", entity.id);
                println!("   Name:        {}", entity.name);
                println!("   Status:      {}", entity.status);
                if let Some(ref desc) = entity.description {
                    println!("   Description: {}", desc);
                }
            }
            EntityCommands::Get { id, format } => {
                let id = crate::core::models::Id::parse(id)?;
                let entity = engine.get_entity(id)?;

                match format {
                    OutputFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&entity)?);
                    }
                    OutputFormat::Yaml => {
                        println!("{}", serde_yaml::to_string(&entity)?);
                    }
                    OutputFormat::Table | OutputFormat::Plain => {
                        println!("ID:          {}", entity.id);
                        println!("Name:        {}", entity.name);
                        println!("Status:      {}", entity.status);
                        if let Some(ref desc) = entity.description {
                            println!("Description: {}", desc);
                        }
                        println!("Created:     {}", entity.metadata.created_at);
                        println!("Updated:     {}", entity.metadata.updated_at);
                        println!("Version:     {}", entity.metadata.version);
                    }
                }
            }
            EntityCommands::List { status, format } => {
                let status_filter = status.map(|s| s.into());
                let entities = engine.list_entities(status_filter)?;

                if entities.is_empty() {
                    println!("No entities found");
                    return Ok(());
                }

                match format {
                    OutputFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&entities)?);
                    }
                    OutputFormat::Yaml => {
                        println!("{}", serde_yaml::to_string(&entities)?);
                    }
                    OutputFormat::Table => {
                        println!("{:<36} {:<20} {:<10} {:<20}", "ID", "Name", "Status", "Created");
                        println!("{}", "-".repeat(90));
                        for entity in entities {
                            println!(
                                "{:<36} {:<20} {:<10} {}",
                                entity.id.to_string(),
                                truncate(&entity.name, 20),
                                entity.status.to_string(),
                                entity.metadata.created_at.format("%Y-%m-%d %H:%M")
                            );
                        }
                    }
                    OutputFormat::Plain => {
                        for entity in entities {
                            println!("{}  {}  {}", entity.id, entity.name, entity.status);
                        }
                    }
                }
            }
            EntityCommands::Update {
                id,
                name,
                description,
                status,
            } => {
                let id = crate::core::models::Id::parse(id)?;
                let request = UpdateEntityRequest {
                    name: name.clone(),
                    description: description.clone(),
                    status: status.map(|s| s.into()),
                    tags: None,
                };

                let entity = engine.update_entity(id, request)?;
                println!("✅ Updated entity {}:", entity.id);
                println!("   Name:   {}", entity.name);
                println!("   Status: {}", entity.status);
            }
            EntityCommands::Delete { id, force } => {
                let id = crate::core::models::Id::parse(id)?;

                if !force {
                    print!("Are you sure you want to delete entity {}? [y/N] ", id);
                    use std::io::Write;
                    std::io::stdout().flush()?;

                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input)?;

                    if !input.trim().eq_ignore_ascii_case("y") {
                        println!("Cancelled");
                        return Ok(());
                    }
                }

                engine.delete_entity(id)?;
                println!("✅ Deleted entity {}", id);
            }
        }

        Ok(())
    }

    async fn run_cron_command(&self, command: &CronCommands) -> Result<()> {
        use crate::cron::ScheduledJob;
        use serde_json;

        // Use centralized ~/.manta/cron directory
        let jobs_file = crate::dirs::cron_dir().join("cron_jobs.json");

        let mut jobs: Vec<ScheduledJob> = if jobs_file.exists() {
            let content = tokio::fs::read_to_string(&jobs_file).await.unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Vec::new()
        };

        match command {
            CronCommands::List => {
                if jobs.is_empty() {
                    println!("No cron jobs configured");
                    return Ok(());
                }

                println!("{:<20} {:<20} {:<10} {:<10}", "Name", "Schedule", "Status", "Run Count");
                println!("{}", "-".repeat(70));

                for job in &jobs {
                    let status = if job.enabled { "enabled" } else { "disabled" };
                    println!("{:<20} {:<20} {:<10} {}",
                        truncate(&job.name, 20),
                        truncate(&job.schedule, 20),
                        status,
                        job.run_count
                    );
                }
            }
            CronCommands::Add { name, schedule, command: cmd, description } => {
                if jobs.iter().any(|j| j.name == *name) {
                    return Err(crate::error::MantaError::Validation(format!("Job '{}' already exists", name)));
                }

                let job = ScheduledJob::new(
                    uuid::Uuid::new_v4().to_string(),
                    name.clone(),
                    schedule.clone(),
                    cmd.clone(),
                    "cli".to_string()
                );

                jobs.push(job);

                let content = serde_json::to_string_pretty(&jobs)?;
                tokio::fs::write(&jobs_file, content).await?;

                println!("✅ Added cron job '{}' with schedule '{}'", name, schedule);
                if let Some(desc) = description {
                    println!("   Description: {}", desc);
                }
            }
            CronCommands::Remove { name } => {
                let initial_len = jobs.len();
                jobs.retain(|j| j.name != *name);

                if jobs.len() == initial_len {
                    return Err(crate::error::MantaError::NotFound { resource: format!("Job '{}'", name) });
                }

                let content = serde_json::to_string_pretty(&jobs)?;
                tokio::fs::write(&jobs_file, content).await?;

                println!("✅ Removed cron job '{}'", name);
            }
            CronCommands::Enable { name } => {
                if let Some(job) = jobs.iter_mut().find(|j| j.name == *name) {
                    job.enabled = true;
                    let content = serde_json::to_string_pretty(&jobs)?;
                    tokio::fs::write(&jobs_file, content).await?;
                    println!("✅ Enabled cron job '{}'", name);
                } else {
                    return Err(crate::error::MantaError::NotFound { resource: format!("Job '{}'", name) });
                }
            }
            CronCommands::Disable { name } => {
                if let Some(job) = jobs.iter_mut().find(|j| j.name == *name) {
                    job.enabled = false;
                    let content = serde_json::to_string_pretty(&jobs)?;
                    tokio::fs::write(&jobs_file, content).await?;
                    println!("✅ Disabled cron job '{}'", name);
                } else {
                    return Err(crate::error::MantaError::NotFound { resource: format!("Job '{}'", name) });
                }
            }
            CronCommands::Run { name } => {
                if let Some(job) = jobs.iter().find(|j| j.name == *name) {
                    println!("Running cron job '{}'...", name);
                    println!("Command: {}", job.prompt);
                    println!("✅ Simulated execution of cron job '{}'", name);
                } else {
                    return Err(crate::error::MantaError::NotFound { resource: format!("Job '{}'", name) });
                }
            }
            CronCommands::Status => {
                println!("📅 Cron Scheduler Status");
                println!("=======================");
                println!("Total jobs: {}", jobs.len());
                println!("Enabled jobs: {}", jobs.iter().filter(|j| j.enabled).count());
                println!("Jobs file: {:?}", jobs_file);

                if !jobs.is_empty() {
                    println!("\nConfigured jobs:");
                    for job in &jobs {
                        let status = if job.enabled { "✅" } else { "❌" };
                        println!("  {} {} - {} (runs: {})",
                            status,
                            job.name,
                            job.schedule,
                            job.run_count
                        );
                    }
                }
            }
        }

        Ok(())
    }

    async fn run_skill_command(&self, command: &SkillCommands) -> Result<()> {
        use crate::skills::{SkillManager, Skill, TriggerType};

        match command {
            SkillCommands::List { all: _, format } => {
                let mut manager = SkillManager::new().await?;
                let count = manager.initialize().await?;

                let skills = manager.list_skills().await;

                if skills.is_empty() {
                    println!("No skills installed. Use 'manta skill init <name>' to create one.");
                    return Ok(());
                }

                println!("📦 Skills ({} total)\n", count);

                match format {
                    OutputFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&skills)?);
                    }
                    OutputFormat::Yaml => {
                        println!("{}", serde_yaml::to_string(&skills)?);
                    }
                    OutputFormat::Table => {
                        println!("{:<20} {:<8} {:<10} {:<30}", "Name", "Status", "Level", "Description");
                        println!("{}", "-".repeat(70));
                        for skill in skills {
                            let status = if skill.is_eligible { "✅" } else { "⚠️" };
                            let enabled = if skill.enabled { "" } else { " (disabled)" };
                            let desc = truncate(&skill.description, 30);
                            println!("{:<20} {:<8} {:<10} {}{}",
                                skill.name,
                                status,
                                skill.source_level,
                                desc,
                                enabled
                            );
                        }
                    }
                    OutputFormat::Plain => {
                        for skill in skills {
                            println!("{} - {}", skill.name, skill.description);
                        }
                    }
                }
            }
            SkillCommands::Info { name } => {
                let mut manager = SkillManager::new().await?;
                manager.initialize().await?;

                if let Some(skill) = manager.get_skill(name).await {
                    println!("📦 Skill: {} {}", skill.metadata.emoji, skill.name);
                    println!("{}\n", "=".repeat(50));
                    println!("Description: {}", skill.description);
                    println!("Status: {}", if skill.is_eligible { "✅ Eligible" } else { "⚠️ Not eligible" });
                    println!("Enabled: {}", if skill.enabled { "Yes" } else { "No" });
                    println!("Source: {:?}", skill.source_level);
                    println!("File: {:?}", skill.source_path);

                    if !skill.metadata.requires.bins.is_empty() {
                        println!("\nRequired binaries: {:?}", skill.metadata.requires.bins);
                    }
                    if !skill.metadata.requires.env.is_empty() {
                        println!("Required env vars: {:?}", skill.metadata.requires.env);
                    }
                    if !skill.metadata.install.is_empty() {
                        println!("\nInstall specs: {}", skill.metadata.install.len());
                    }
                    if !skill.triggers.is_empty() {
                        println!("\nTriggers:");
                        for trigger in &skill.triggers {
                            println!("  - {:?}: {}", trigger.trigger_type, trigger.pattern);
                        }
                    }
                } else {
                    println!("❌ Skill '{}' not found", name);
                }
            }
            SkillCommands::Install { source, name } => {
                println!("📦 Installing skill from '{}'...", source);
                let path = std::path::Path::new(source);

                if !path.exists() {
                    println!("❌ Source path '{}' does not exist", source);
                    return Ok(());
                }

                let skill_name = name.clone().unwrap_or_else(|| {
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string()
                });

                let storage = crate::skills::SkillStorage::new()?;
                let dest = storage.install_to_user(path, &skill_name).await?;

                println!("✅ Installed skill '{}' to {:?}", skill_name, dest);
                println!("   Run 'manta skill setup {}' to install dependencies", skill_name);
            }
            SkillCommands::Uninstall { name, force } => {
                if !force {
                    print!("Are you sure you want to uninstall skill '{}'? [y/N] ", name);
                    use std::io::Write;
                    std::io::stdout().flush()?;

                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input)?;

                    if !input.trim().eq_ignore_ascii_case("y") {
                        println!("Cancelled");
                        return Ok(());
                    }
                }

                let storage = crate::skills::SkillStorage::new()?;
                storage.uninstall_from_user(name).await?;
                println!("✅ Uninstalled skill '{}'", name);
            }
            SkillCommands::Enable { name } => {
                let mut manager = SkillManager::new().await?;
                manager.initialize().await?;
                manager.set_skill_enabled(name, true).await?;
                println!("✅ Enabled skill '{}'", name);
            }
            SkillCommands::Disable { name } => {
                let mut manager = SkillManager::new().await?;
                manager.initialize().await?;
                manager.set_skill_enabled(name, false).await?;
                println!("✅ Disabled skill '{}'", name);
            }
            SkillCommands::Setup { name } => {
                let mut manager = SkillManager::new().await?;
                manager.initialize().await?;

                if let Some(skill_name) = name {
                    println!("📦 Setting up skill '{}'...", skill_name);
                    let results = manager.install_skill(skill_name).await?;

                    for result in results {
                        match result {
                            crate::skills::InstallResult::Installed { spec } => {
                                println!("✅ Installed: {:?}", spec);
                            }
                            crate::skills::InstallResult::AlreadyPresent { spec } => {
                                println!("✓ Already present: {:?}", spec);
                            }
                            crate::skills::InstallResult::Failed { spec, error } => {
                                println!("❌ Failed to install {:?}: {}", spec, error);
                            }
                            crate::skills::InstallResult::Skipped { spec, reason } => {
                                println!("⚠️ Skipped {:?}: {}", spec, reason);
                            }
                        }
                    }
                } else {
                    println!("📦 Setting up all skills...");
                    let skills = manager.list_skills().await;
                    for skill in skills {
                        if skill.is_eligible {
                            println!("\nSetting up '{}'...", skill.name);
                            let _ = manager.install_skill(&skill.name).await;
                        }
                    }
                }
            }
            SkillCommands::Init { name, description } => {
                let desc = description.clone().unwrap_or_else(|| format!("{} skill", name));

                let skill = Skill::new(name, &desc, "")
                    .with_trigger(TriggerType::Keyword, name);

                let manager = SkillManager::new().await?;
                manager.create_skill(&skill).await?;

                println!("✅ Created skill template '{}'", name);
                println!("   Edit: ~/.manta/skills/{}/SKILL.md", name);
            }
        }

        Ok(())
    }

    async fn run_agent_command(&self, command: &AgentCommands) -> Result<()> {
        

        // Use centralized ~/.manta/agents directory
        let agents_dir = crate::dirs::agents_dir();

        tokio::fs::create_dir_all(&agents_dir).await.ok();

        match command {
            AgentCommands::Create { name, display_name, role, style, prompt, format: output_format } => {
                let agent_dir = agents_dir.join(name);

                if agent_dir.exists() {
                    return Err(crate::error::MantaError::Validation(
                        format!("Agent '{}' already exists. Use 'manta agent remove {}' to delete it first.", name, name)
                    ));
                }

                // Create agent directory
                tokio::fs::create_dir_all(&agent_dir).await?;

                let display = display_name.clone().unwrap_or_else(|| name.clone());
                let role_text = role.clone().unwrap_or_else(|| "AI Assistant".to_string());
                let style_text = style.clone();
                let prompt_text = prompt.clone().unwrap_or_else(||
                    format!("You are {}, a helpful AI assistant.", display)
                );

                // Create IDENTITY.md
                let identity_content = format!(r#"# Agent Identity

## Name
{display}

## Role
{role_text}

## Communication Style
{style_text}

## Created
{date}
"#, date = chrono::Local::now().format("%Y-%m-%d %H:%M"));

                // Create SOUL.md
                let soul_content = format!(r#"# Agent Soul

## Core Values
- Helpfulness: Always strive to be useful
- Honesty: Admit when you don't know something
- Clarity: Communicate in a {style_text} manner

## Behavioral Guidelines
- Be {style_text} in all interactions
- Focus on the user's goals
- Ask clarifying questions when needed

## Expertise
{role_text}
"#);

                // Create BOOTSTRAP.md
                let bootstrap_content = format!(r#"# Bootstrap Configuration

## System Prompt
{prompt_text}

## Initial Greeting
Hello! I'm {display}, your {role_text}. How can I help you today?

## Startup Behavior
- Load context from memory
- Check for pending tasks
- Await user input
"#);

                // Write files based on format
                match output_format.as_str() {
                    "yaml" | "json" => {
                        // For structured formats, create a single agent.yaml file
                        let agent_yaml = format!(r#"name: {display}
role: {role_text}
style: {style_text}
created: {date}
system_prompt: |
  {prompt_text}
"#, date = chrono::Local::now().format("%Y-%m-%d %H:%M"));
                        tokio::fs::write(agent_dir.join("agent.yaml"), agent_yaml).await?;

                        // Also write markdown versions for editing
                        tokio::fs::write(agent_dir.join("SOUL.md"), soul_content).await?;
                        tokio::fs::write(agent_dir.join("IDENTITY.md"), identity_content).await?;
                        tokio::fs::write(agent_dir.join("BOOTSTRAP.md"), bootstrap_content).await?;
                    }
                    _ => {
                        // Default markdown format (OpenClaw-style)
                        tokio::fs::write(agent_dir.join("SOUL.md"), soul_content).await?;
                        tokio::fs::write(agent_dir.join("IDENTITY.md"), identity_content).await?;
                        tokio::fs::write(agent_dir.join("BOOTSTRAP.md"), bootstrap_content).await?;
                    }
                }

                println!("✅ Created agent personality '{}'", name);
                println!("   Location: {}", agent_dir.display());
                println!("   Display Name: {}", display);
                println!("   Role: {}", role_text);
                println!("   Style: {}", style_text);
                println!();
                println!("   Files created:");
                println!("   - SOUL.md (personality & values)");
                println!("   - IDENTITY.md (name & role)");
                println!("   - BOOTSTRAP.md (startup behavior)");
                println!();
                println!("   To activate: manta agent set {}", name);
                println!("   To edit: manta agent edit {}", name);
            }

            AgentCommands::Remove { name, force } => {
                let agent_dir = agents_dir.join(name);

                if !agent_dir.exists() {
                    return Err(crate::error::MantaError::Validation(
                        format!("Agent '{}' not found", name)
                    ));
                }

                if !force {
                    print!("Are you sure you want to remove agent '{}'? [y/N] ", name);
                    use std::io::Write;
                    std::io::stdout().flush()?;

                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input)?;

                    if !input.trim().eq_ignore_ascii_case("y") {
                        println!("Cancelled");
                        return Ok(());
                    }
                }

                tokio::fs::remove_dir_all(&agent_dir).await?;
                println!("✅ Removed agent '{}'", name);
            }

            AgentCommands::List { verbose } => {
                let mut entries = tokio::fs::read_dir(&agents_dir).await?;
                let mut agents = Vec::new();

                while let Some(entry) = entries.next_entry().await? {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();

                        // Try to read IDENTITY.md for display name
                        let identity_path = path.join("IDENTITY.md");
                        let display_name = if identity_path.exists() {
                            match tokio::fs::read_to_string(&identity_path).await {
                                Ok(content) => {
                                    // Extract name from "## Name" section
                                    content.lines()
                                        .skip_while(|l| !l.starts_with("## Name"))
                                        .nth(1)
                                        .map(|l| l.trim().to_string())
                                        .unwrap_or_else(|| name.clone())
                                }
                                Err(_) => name.clone()
                            }
                        } else {
                            name.clone()
                        };

                        agents.push((name, display_name));
                    }
                }

                if agents.is_empty() {
                    println!("No agent personalities found.");
                    println!("Create one with: manta agent create <name>");
                    return Ok(());
                }

                if *verbose {
                    println!("{:<20} {:<30}", "Name", "Display Name");
                    println!("{}", "-".repeat(55));
                    for (name, display) in &agents {
                        println!("{:<20} {:<30}", name, display);
                    }
                } else {
                    for (name, display) in &agents {
                        println!("{} ({})", name, display);
                    }
                }

                println!();
                println!("Total: {} agent(s)", agents.len());
            }

            AgentCommands::Set { name } => {
                if name == "default" {
                    // Reset to default - remove active agent symlink/file
                    let active_file = crate::dirs::config_dir().join(".active_agent");
                    if active_file.exists() {
                        tokio::fs::remove_file(&active_file).await?;
                    }
                    println!("✅ Reset to default agent personality");
                    return Ok(());
                }

                let agent_dir = agents_dir.join(name);

                if !agent_dir.exists() {
                    return Err(crate::error::MantaError::Validation(
                        format!("Agent '{}' not found. Use 'manta agent create {}' to create it.", name, name)
                    ));
                }

                // Create active agent marker
                let active_file = crate::dirs::config_dir().join(".active_agent");
                tokio::fs::write(&active_file, name).await?;

                println!("✅ Set active agent personality to '{}'", name);
                println!("   Restart Manta daemon to apply changes.");
            }

            AgentCommands::Show => {
                let active_file = crate::dirs::config_dir().join(".active_agent");

                let active_agent = if active_file.exists() {
                    tokio::fs::read_to_string(&active_file).await.ok()
                } else {
                    None
                };

                if let Some(name) = active_agent {
                    let agent_dir = agents_dir.join(&name);
                    println!("🤖 Active Agent: {}", name);
                    println!();

                    // Show IDENTITY.md
                    let identity_path = agent_dir.join("IDENTITY.md");
                    if identity_path.exists() {
                        match tokio::fs::read_to_string(&identity_path).await {
                            Ok(content) => {
                                println!("{}", content);
                            }
                            Err(_) => println!("   (Could not read IDENTITY.md)"),
                        }
                    }

                    println!("Location: {}", agent_dir.display());
                } else {
                    println!("Using default agent personality (no custom agent set)");
                    println!("Create one with: manta agent create <name>");
                }
            }

            AgentCommands::Edit { name, file } => {
                let agent_dir = agents_dir.join(name);

                if !agent_dir.exists() {
                    return Err(crate::error::MantaError::Validation(
                        format!("Agent '{}' not found", name)
                    ));
                }

                let files_to_edit: Vec<std::path::PathBuf> = match file.as_str() {
                    "soul" => vec![agent_dir.join("SOUL.md")],
                    "identity" => vec![agent_dir.join("IDENTITY.md")],
                    "bootstrap" => vec![agent_dir.join("BOOTSTRAP.md")],
                    "all" => vec![
                        agent_dir.join("SOUL.md"),
                        agent_dir.join("IDENTITY.md"),
                        agent_dir.join("BOOTSTRAP.md"),
                    ],
                    _ => {
                        return Err(crate::error::MantaError::Validation(
                            format!("Unknown file '{}'. Use: soul, identity, bootstrap, or all", file)
                        ));
                    }
                };

                for file_path in files_to_edit {
                    if file_path.exists() {
                        println!("Editing: {}", file_path.display());
                        // Open with default editor (simplified - in real impl, use $EDITOR)
                        println!("   (Open this file in your editor to modify)");
                    } else {
                        println!("   File not found: {}", file_path.display());
                    }
                }

                println!();
                println!("Tip: Set $EDITOR environment variable to open files automatically.");
            }

            AgentCommands::InitWorkspace { force } => {
                // Find workspace root
                let cwd = std::env::current_dir()?;
                let mut current = cwd.as_path();
                let mut workspace_root: Option<&std::path::Path> = None;

                loop {
                    let markers = [".manta-workspace", ".git", "manta.workspace.toml"];
                    for marker in &markers {
                        if current.join(marker).exists() {
                            workspace_root = Some(current);
                            break;
                        }
                    }
                    if workspace_root.is_some() {
                        break;
                    }
                    match current.parent() {
                        Some(parent) => current = parent,
                        None => break,
                    }
                }

                let workspace_dir = match workspace_root {
                    Some(root) => root.join(".manta").join("memory"),
                    None => {
                        // No workspace found, create in current directory
                        println!("ℹ️ No workspace marker found (.git, .manta-workspace, or manta.workspace.toml)");
                        println!("   Creating workspace-level memory in current directory: .manta/memory/");
                        std::env::current_dir()?.join(".manta").join("memory")
                    }
                };

                // Create directory
                tokio::fs::create_dir_all(&workspace_dir).await?;

                let files = [
                    ("SOUL.md", "# SOUL\n\nCore personality for this workspace.\n\n## Values\n- Workspace-specific values\n- Project priorities\n\n## Communication Style\n- How the agent should communicate\n"),
                    ("IDENTITY.md", "# IDENTITY\n\nAgent identity for this workspace.\n\n## Name\nManta\n\n## Role\nAI Assistant specialized for this project\n\n## Purpose\nHelp with this specific codebase and its needs.\n"),
                    ("BOOTSTRAP.md", "# BOOTSTRAP\n\nInitial behavior for this workspace.\n\n## Context Awareness\n- Understand the project structure\n- Be familiar with the tech stack\n\n## Session Start\n- Check for existing project context\n- Review recent changes if applicable\n"),
                ];

                for (filename, default_content) in &files {
                    let file_path = workspace_dir.join(filename);
                    if file_path.exists() && !force {
                        println!("ℹ️ Skipping {} (already exists, use --force to overwrite)", filename);
                    } else {
                        tokio::fs::write(&file_path, default_content).await?;
                        println!("✅ Created {}", file_path.display());
                    }
                }

                println!();
                println!("📝 Workspace-level memory files initialized!");
                println!("   Location: {}", workspace_dir.display());
                println!();
                println!("These files will be used when running manta in this workspace.");
                println!("They override the global ~/.manta/memory-files/ settings.");
            }
        }

        Ok(())
    }

    fn show_config(&self, config: &Config, format: ConfigFormat) -> Result<()> {
        match format {
            ConfigFormat::Toml => {
                // Remove the app field for serialization
                let toml_str = toml::to_string_pretty(&config)?;
                println!("{}", toml_str);
            }
            ConfigFormat::Json => {
                println!("{}", serde_json::to_string_pretty(&config)?);
            }
            ConfigFormat::Yaml => {
                println!("{}", serde_yaml::to_string(&config)?);
            }
        }
        Ok(())
    }

    async fn run_health_check(&self, config: &Config) -> Result<()> {
        println!("🏥 Health Check");
        println!("===============");
        println!();

        // Check configuration
        println!("✅ Configuration loaded");
        println!("   Server: {}", config.server_addr());
        println!("   Log level: {}", config.logging.level);
        println!("   Storage: {:?}", config.storage.storage_type);

        // Test engine
        let engine = Engine::new();
        let count = engine.entity_count()?;
        println!("✅ Engine operational ({} entities)", count);

        println!();
        println!("All systems operational!");

        Ok(())
    }

    /// Load skills and build the skills prompt
    pub async fn load_skills_prompt() -> Result<Option<String>> {
        use crate::skills::SkillManager;

        let mut manager = SkillManager::new().await?;
        let count = manager.initialize().await?;

        if count == 0 {
            return Ok(None);
        }

        let skills = manager.list_skills().await;
        let mut eligible_skills = Vec::new();

        for skill in skills {
            if skill.is_eligible && skill.enabled {
                eligible_skills.push(skill.to_prompt_section());
            }
        }

        if eligible_skills.is_empty() {
            return Ok(None);
        }

        let prompt = eligible_skills.join("\n\n---\n\n");
        Ok(Some(prompt))
    }

    async fn run_chat(
        &self,
        _config: &Config,
        conversation_id: Option<String>,
        single_message: Option<String>,
    ) -> Result<()> {
        use crate::client::check_daemon;
        use crate::memory::ChatHistoryStore;

        println!("🤖 Manta AI Assistant");
        println!("=====================");
        println!();

        // Check if daemon is running
        let client = match check_daemon().await {
            Ok(client) => client,
            Err(e) => {
                println!("❌ {}", e);
                return Ok(());
            }
        };

        println!("✅ Connected to daemon");
        println!();

        // Use provided conversation ID, or get last conversation, or generate new one
        let conversation_id = match conversation_id {
            Some(id) => id,
            None => {
                // Try to get the last conversation from the database
                let db_path = crate::dirs::default_memory_db();
                let db_url = format!("sqlite:{}", db_path.display());

                match crate::memory::SqliteMemoryStore::new(&db_url).await {
                    Ok(store) => {
                        match store.get_last_conversation("user").await {
                            Ok(Some(last_conv)) => {
                                println!("📱 Resuming conversation: {}", last_conv);
                                last_conv
                            }
                            _ => {
                                let new_id = crate::channels::ConversationId::generate().to_string();
                                println!("📱 Starting new conversation: {}", new_id);
                                new_id
                            }
                        }
                    }
                    Err(_) => {
                        let new_id = crate::channels::ConversationId::generate().to_string();
                        println!("📱 Starting new conversation: {}", new_id);
                        new_id
                    }
                }
            }
        };
        println!();

        // Single message mode
        if let Some(message) = single_message {
            match client.chat_ws(&message, Some(&conversation_id)).await {
                Ok(response) => println!("🤖 {}", response.response),
                Err(e) => println!("❌ Error: {}", e),
            }
            return Ok(());
        }

        // Interactive mode
        println!("Type 'exit' to quit, 'help' for commands, '/new' for new session\n");

        // Check if we're running in a TTY
        let is_tty = std::io::stdin().is_terminal();

        if is_tty {
            // Terminal UI mode
            Self::run_interactive_daemon_chat(client, conversation_id).await
        } else {
            // Simple line-based mode for piped input
            Self::run_simple_daemon_chat(client, conversation_id).await
        }
    }

    /// Run interactive chat with daemon (TTY mode)
    async fn run_interactive_daemon_chat(
        client: crate::client::DaemonClient,
        mut conversation_id: String,
    ) -> Result<()> {
        use std::io::{self, Write};

        print!("💬 You > ");
        io::stdout().flush()?;

        loop {
            let mut input = String::new();
            match io::stdin().read_line(&mut input) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    eprintln!("❌ Input error: {}", e);
                    break;
                }
            }

            let input = input.trim();

            if input.is_empty() {
                print!("💬 You > ");
                io::stdout().flush()?;
                continue;
            }

            match input.to_lowercase().as_str() {
                "exit" | "quit" => {
                    println!("👋 Goodbye!");
                    break;
                }
                "help" => {
                    println!("📋 Commands: /new, help, exit");
                    println!("  /new - Start a new conversation");
                    println!("  help - Show this help");
                    println!("  exit - Exit the chat");
                    print!("💬 You > ");
                    io::stdout().flush()?;
                    continue;
                }
                "/new" => {
                    conversation_id = crate::channels::ConversationId::generate().to_string();
                    println!("🆕 Started new conversation: {}", conversation_id);
                    print!("💬 You > ");
                    io::stdout().flush()?;
                    continue;
                }
                _ => {}
            }

            eprint!("🤖 Thinking...");
            io::stderr().flush()?;

            match client.chat_ws(input, Some(&conversation_id)).await {
                Ok(response) => {
                    eprint!("\r\x1B[2K");
                    println!("🤖 {}", response.response.trim().replace('\n', " "));
                }
                Err(e) => {
                    eprint!("\r\x1B[2K");
                    eprintln!("❌ Error: {}", e);
                }
            }

            print!("💬 You > ");
            io::stdout().flush()?;
        }

        Ok(())
    }

    /// Run simple chat with daemon (non-TTY mode)
    async fn run_simple_daemon_chat(
        client: crate::client::DaemonClient,
        mut conversation_id: String,
    ) -> Result<()> {
        use tokio::io::{AsyncBufReadExt, BufReader, stdin};
        use std::io::Write;

        println!("🤖 Manta Terminal Chat - Type 'exit' to quit, '/new' for new session");

        let stdin = BufReader::new(stdin());
        let mut lines = stdin.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let input = line.trim();

            if input.is_empty() {
                print!("💬 You > ");
                std::io::stdout().flush()?;
                continue;
            }

            match input.to_lowercase().as_str() {
                "exit" | "quit" => {
                    println!("👋 Goodbye!");
                    break;
                }
                "/new" => {
                    conversation_id = crate::channels::ConversationId::generate().to_string();
                    println!("🆕 Started new conversation: {}", conversation_id);
                    print!("💬 You > ");
                    std::io::stdout().flush()?;
                    continue;
                }
                _ => {}
            }

            match client.chat_ws(input, Some(&conversation_id)).await {
                Ok(response) => {
                    println!("🤖 {}", response.response.trim().replace('\n', " "));
                    print!("💬 You > ");
                    std::io::stdout().flush()?;
                }
                Err(e) => {
                    eprintln!("❌ Error: {}", e);
                    print!("💬 You > ");
                    std::io::stdout().flush()?;
                }
            }
        }

        Ok(())
    }

    /// Run web terminal server (connects to daemon)
    async fn run_web(&self, _config: &Config, port: u16) -> Result<()> {
        use crate::client::check_daemon;
        use crate::web::start_web_terminal_with_daemon;

        println!("🌐 Starting Manta Web Terminal");
        println!("================================");

        // Check if daemon is running
        let client = match check_daemon().await {
            Ok(client) => client,
            Err(e) => {
                println!("❌ {}", e);
                return Ok(());
            }
        };

        println!("✅ Connected to daemon");
        println!("🌐 Starting web terminal on port {}", port);
        println!();
        println!("Open http://localhost:{} in your browser", port);

        // Start web terminal that proxies to daemon
        start_web_terminal_with_daemon(client, port).await
    }

    /// Start the Manta daemon
    async fn run_start_daemon(&self, host: &str, port: u16, web_port: u16, foreground: bool) -> Result<()> {
        use crate::daemon::{DaemonManager, DaemonConfig};

        // Ensure directories exist
        crate::dirs::init().await?;

        let config = DaemonConfig {
            host: host.to_string(),
            port,
            web_port,
            pid_file: crate::dirs::pid_file(),
        };

        let daemon = DaemonManager::new(config)?;

        if foreground {
            println!("🚀 Starting Manta daemon in foreground...");
            println!("   Host: {}", host);
            println!("   API Port: {}", port);
            println!("   Web Port: {}", web_port);
            println!("   Press Ctrl+C to stop\n");
            daemon.run_foreground().await
        } else {
            println!("🚀 Starting Manta daemon in background...");
            daemon.start().await
        }
    }

    /// Stop the Manta daemon
    async fn run_stop_daemon(&self, force: bool) -> Result<()> {
        use crate::daemon::{DaemonManager, DaemonConfig};

        let config = DaemonConfig {
            host: "127.0.0.1".to_string(),
            port: 18080,
            web_port: 18081,
            pid_file: crate::dirs::pid_file(),
        };

        let daemon = DaemonManager::new(config)?;

        if force {
            println!("🛑 Force stopping Manta daemon...");
            daemon.stop_force().await
        } else {
            println!("🛑 Stopping Manta daemon...");
            daemon.stop().await
        }
    }

    /// Check daemon status
    async fn run_daemon_status(&self) -> Result<()> {
        use crate::daemon::{DaemonManager, DaemonConfig};

        let config = DaemonConfig {
            host: "127.0.0.1".to_string(),
            port: 18080,
            web_port: 18081,
            pid_file: crate::dirs::pid_file(),
        };

        let daemon = DaemonManager::new(config)?;
        daemon.status().await
    }

    /// Show daemon logs
    async fn run_logs(&self, lines: usize, follow: bool) -> Result<()> {
        use crate::logs::{show_logs, tail_logs};

        if follow {
            tail_logs(lines).await
        } else {
            show_logs(lines).await
        }
    }

    /// Run channel commands
    async fn run_channel_command(&self, command: &ChannelCommands) -> Result<()> {
        match command {
            ChannelCommands::Add { channel, token, foreground } => {
                self.run_channel_add(*channel, token.clone(), *foreground).await
            }
            ChannelCommands::Remove { channel } => {
                self.run_channel_remove(*channel).await
            }
            ChannelCommands::List { all } => {
                self.run_channel_list(*all).await
            }
            ChannelCommands::Status { channel } => {
                self.run_channel_status(*channel).await
            }
            ChannelCommands::Test { channel } => {
                self.run_channel_test(*channel).await
            }
        }
    }

    /// Add/connect a channel (Telegram, Discord, Slack, WhatsApp, QQ, Lark)
    async fn run_channel_add(&self, channel: ChannelType, token: Option<String>, foreground: bool) -> Result<()> {
        match channel {
            ChannelType::Telegram => {
                self.start_telegram_channel(token, foreground).await
            }
            ChannelType::Discord => {
                println!("🚧 Discord channel support coming soon!");
                println!("   To use Discord, build with: cargo build --features discord");
                Ok(())
            }
            ChannelType::Slack => {
                println!("🚧 Slack channel support coming soon!");
                println!("   To use Slack, build with: cargo build --features slack");
                Ok(())
            }
            ChannelType::Whatsapp => {
                println!("🚧 WhatsApp channel support coming soon!");
                println!("   To use WhatsApp, build with: cargo build --features whatsapp");
                Ok(())
            }
            ChannelType::Qq => {
                println!("🚧 QQ channel support coming soon!");
                println!("   To use QQ, build with: cargo build --features qq");
                Ok(())
            }
            ChannelType::Lark => {
                println!("🚧 Lark/Feishu channel support coming soon!");
                println!("   To use Lark/Feishu, build with: cargo build --features lark");
                Ok(())
            }
        }
    }

    /// Start Telegram channel
    #[cfg(feature = "telegram")]
    async fn start_telegram_channel(&self, token: Option<String>, foreground: bool) -> Result<()> {
        use crate::channels::{Channel, TelegramChannel, TelegramConfig};
        use crate::agent::{AgentConfig, AgentBuilder};
        use crate::tools::ToolRegistry;

        // Get token from args or environment
        let token = match token {
            Some(t) => t,
            None => std::env::var("TELEGRAM_BOT_TOKEN")
                .map_err(|_| crate::error::ConfigError::Missing(
                    "TELEGRAM_BOT_TOKEN environment variable or --token argument".to_string()
                ))?,
        };

        println!("🚀 Starting Telegram bot...");

        // Create agent with tools
        let tools = ToolRegistry::new();
        let _agent = Arc::new(AgentBuilder::new()
            .config(AgentConfig::default())
            .tools(Arc::new(tools))
            .build()?);

        // Create and start Telegram channel
        let config = TelegramConfig::new(token);
        let channel = TelegramChannel::new(config);

        if foreground {
            println!("   Running in foreground mode (Press Ctrl+C to stop)");
            channel.start().await?;
        } else {
            println!("   Starting in background...");
            // TODO: Implement background mode with PID file
            channel.start().await?;
        }

        Ok(())
    }

    /// Telegram support not compiled in
    #[cfg(not(feature = "telegram"))]
    async fn start_telegram_channel(&self, _token: Option<String>, _foreground: bool) -> Result<()> {
        println!("❌ Telegram support not compiled in.");
        println!("   Build with: cargo build --features telegram");
        Ok(())
    }

    /// Remove/disconnect a channel
    async fn run_channel_remove(&self, channel: ChannelType) -> Result<()> {
        match channel {
            ChannelType::Telegram => {
                println!("🗑️  Removing Telegram bot...");
                // TODO: Implement channel PID file tracking
                println!("   Note: Use 'pkill manta' or Ctrl+C if running in foreground");
                Ok(())
            }
            ChannelType::Discord => {
                println!("🗑️  Removing Discord bot...");
                println!("   Note: Use 'pkill manta' or Ctrl+C if running in foreground");
                Ok(())
            }
            ChannelType::Slack => {
                println!("🗑️  Removing Slack bot...");
                println!("   Note: Use 'pkill manta' or Ctrl+C if running in foreground");
                Ok(())
            }
            ChannelType::Whatsapp => {
                println!("🗑️  Removing WhatsApp bot...");
                println!("   Note: Use 'pkill manta' or Ctrl+C if running in foreground");
                Ok(())
            }
            ChannelType::Qq => {
                println!("🗑️  Removing QQ bot...");
                println!("   Note: Use 'pkill manta' or Ctrl+C if running in foreground");
                Ok(())
            }
            ChannelType::Lark => {
                println!("🗑️  Removing Lark/Feishu bot...");
                println!("   Note: Use 'pkill manta' or Ctrl+C if running in foreground");
                Ok(())
            }
        }
    }

    /// List configured channels
    async fn run_channel_list(&self, all: bool) -> Result<()> {
        println!("📱 Manta Channels");
        println!("=================");
        println!();

        // Check feature flags
        #[cfg(feature = "telegram")]
        println!("✅ Telegram   - Available (compile-time enabled)");
        #[cfg(not(feature = "telegram"))]
        println!("❌ Telegram   - Not compiled (use --features telegram)");

        #[cfg(feature = "discord")]
        println!("✅ Discord    - Available (compile-time enabled)");
        #[cfg(not(feature = "discord"))]
        println!("❌ Discord    - Not compiled (use --features discord)");

        #[cfg(feature = "slack")]
        println!("✅ Slack      - Available (compile-time enabled)");
        #[cfg(not(feature = "slack"))]
        println!("❌ Slack      - Not compiled (use --features slack)");

        #[cfg(feature = "whatsapp")]
        println!("✅ WhatsApp   - Available (compile-time enabled)");
        #[cfg(not(feature = "whatsapp"))]
        println!("❌ WhatsApp   - Not compiled (use --features whatsapp)");

        #[cfg(feature = "qq")]
        println!("✅ QQ         - Available (compile-time enabled)");
        #[cfg(not(feature = "qq"))]
        println!("❌ QQ         - Not compiled (use --features qq)");

        #[cfg(feature = "lark")]
        println!("✅ Lark       - Available (compile-time enabled)");
        #[cfg(not(feature = "lark"))]
        println!("❌ Lark       - Not compiled (use --features lark)");

        println!();
        println!("Available commands:");
        println!("  manta channel add <CHANNEL> --token <TOKEN>");
        println!("  manta channel add telegram (with TELEGRAM_BOT_TOKEN env var)");
        println!("  manta channel status <CHANNEL>");
        println!("  manta channel remove <CHANNEL>");
        println!();
        println!("Channels: telegram, discord, slack, whatsapp, qq, lark");

        let _ = all; // Silence warning for now
        Ok(())
    }

    /// Check channel status
    async fn run_channel_status(&self, channel: Option<ChannelType>) -> Result<()> {
        match channel {
            Some(ChannelType::Telegram) => {
                println!("📱 Telegram Bot Status");
                println!("======================");
                println!("Status: Unknown (not implemented yet)");
                // TODO: Check if process is running via PID file
            }
            Some(ChannelType::Discord) => {
                println!("🚧 Discord support coming soon!");
            }
            Some(ChannelType::Slack) => {
                println!("🚧 Slack support coming soon!");
            }
            Some(ChannelType::Whatsapp) => {
                println!("🚧 WhatsApp support coming soon!");
            }
            Some(ChannelType::Qq) => {
                println!("🚧 QQ support coming soon!");
            }
            Some(ChannelType::Lark) => {
                println!("🚧 Lark/Feishu support coming soon!");
            }
            None => {
                // Show all channels
                println!("📱 Channel Status");
                println!("=================");
                println!("Telegram: Not checked (use 'manta channel status telegram')");
                println!("Discord:  Not available");
                println!("Slack:    Not available");
                println!("WhatsApp: Not available");
                println!("QQ:       Not available");
                println!("Lark:     Not available");
            }
        }
        Ok(())
    }

    /// Test channel configuration
    async fn run_channel_test(&self, channel: ChannelType) -> Result<()> {
        match channel {
            ChannelType::Telegram => {
                println!("🧪 Testing Telegram configuration...");

                // Check for token
                match std::env::var("TELEGRAM_BOT_TOKEN") {
                    Ok(_) => println!("✅ TELEGRAM_BOT_TOKEN environment variable found"),
                    Err(_) => println!("⚠️  TELEGRAM_BOT_TOKEN not set (will need --token)"),
                }

                #[cfg(feature = "telegram")]
                println!("✅ Telegram feature compiled in");
                #[cfg(not(feature = "telegram"))]
                println!("❌ Telegram feature not compiled (rebuild with --features telegram)");

                println!();
                println!("To add/connect the bot:");
                println!("  export TELEGRAM_BOT_TOKEN='your_token_here'");
                println!("  manta channel add telegram");
            }
            ChannelType::Discord => {
                println!("🧪 Testing Discord configuration...");
                #[cfg(feature = "discord")]
                println!("✅ Discord feature compiled in");
                #[cfg(not(feature = "discord"))]
                println!("❌ Discord feature not compiled (rebuild with --features discord)");
            }
            ChannelType::Slack => {
                println!("🧪 Testing Slack configuration...");
                #[cfg(feature = "slack")]
                println!("✅ Slack feature compiled in");
                #[cfg(not(feature = "slack"))]
                println!("❌ Slack feature not compiled (rebuild with --features slack)");
            }
            ChannelType::Whatsapp => {
                println!("🧪 Testing WhatsApp configuration...");
                #[cfg(feature = "whatsapp")]
                println!("✅ WhatsApp feature compiled in");
                #[cfg(not(feature = "whatsapp"))]
                println!("❌ WhatsApp feature not compiled (rebuild with --features whatsapp)");
            }
            ChannelType::Qq => {
                println!("🧪 Testing QQ configuration...");
                #[cfg(feature = "qq")]
                println!("✅ QQ feature compiled in");
                #[cfg(not(feature = "qq"))]
                println!("❌ QQ feature not compiled (rebuild with --features qq)");
            }
            ChannelType::Lark => {
                println!("🧪 Testing Lark/Feishu configuration...");
                #[cfg(feature = "lark")]
                println!("✅ Lark feature compiled in");
                #[cfg(not(feature = "lark"))]
                println!("❌ Lark feature not compiled (rebuild with --features lark)");
            }
        }
        Ok(())
    }

    /// Run as an assistant subprocess (internal use)
    async fn run_assistant_process(&self, config_path: &PathBuf) -> Result<()> {
        use crate::assistants::{AssistantConfig, AssistantType};
        use crate::assistants::process::IpcMessage;
        use crate::agent::{AgentConfig, AgentBuilder};
        use crate::tools::{ToolRegistry, ShellTool, FileReadTool, FileWriteTool, FileEditTool, GlobTool, TodoTool, WebSearchTool, WebFetchTool, CronTool};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, stdin, stdout};
        

        // Read environment variables set by parent
        let assistant_id = std::env::var("MANTA_ASSISTANT_ID")
            .unwrap_or_else(|_| "unknown".to_string());
        let assistant_name = std::env::var("MANTA_ASSISTANT_NAME")
            .unwrap_or_else(|_| "Assistant".to_string());
        let assistant_type_str = std::env::var("MANTA_ASSISTANT_TYPE")
            .unwrap_or_else(|_| "specialist".to_string());
        let _parent_id = std::env::var("MANTA_PARENT_ASSISTANT_ID").ok();

        // Parse assistant type
        let assistant_type = match assistant_type_str.as_str() {
            "researcher" => AssistantType::Researcher,
            "code_reviewer" => AssistantType::CodeReviewer,
            "scheduler" => AssistantType::Scheduler,
            "social" => AssistantType::Social,
            s if s.starts_with("specialist:") => {
                AssistantType::Specialist(s.strip_prefix("specialist:").unwrap_or(s).to_string())
            }
            _ => AssistantType::Specialist(assistant_type_str.clone()),
        };

        // Load configuration
        let config_content = tokio::fs::read_to_string(config_path).await
            .map_err(|e| crate::error::MantaError::Internal(
                format!("Failed to read assistant config: {}", e)
            ))?;
        let assistant_config: AssistantConfig = serde_yaml::from_str(&config_content)
            .map_err(|e| crate::error::MantaError::Internal(
                format!("Failed to parse assistant config: {}", e)
            ))?;

        // Set up logging for this assistant
        eprintln!("🤖 Assistant '{}' starting (ID: {}, Type: {})",
            assistant_name, assistant_id, assistant_type);

        // Create provider from environment
        let base_url = std::env::var("MANTA_BASE_URL")
            .or_else(|_| std::env::var("OPENAI_BASE_URL"))
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let api_key = std::env::var("MANTA_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .map_err(|_| crate::error::MantaError::Validation(
                "MANTA_API_KEY or OPENAI_API_KEY environment variable required".to_string()
            ))?;
        let model = std::env::var("MANTA_MODEL")
            .unwrap_or_else(|_| "gpt-4o-mini".to_string());

        // Create provider
        let provider: Arc<dyn crate::providers::Provider> = {
            use crate::providers::openai::OpenAiProvider;
            Arc::new(OpenAiProvider::with_base_url(api_key, base_url)?.with_model(model))
        };

        // Create tool registry with tools based on assistant type
        let mut tool_registry = ToolRegistry::new();
        let tools = assistant_config.effective_tools();
        for tool_name in tools {
            match tool_name.as_str() {
                "shell" => tool_registry.register(Box::new(ShellTool::new())),
                "file_read" => tool_registry.register(Box::new(FileReadTool::new())),
                "file_write" => tool_registry.register(Box::new(FileWriteTool::new())),
                "file_edit" => tool_registry.register(Box::new(FileEditTool::new())),
                "glob" => tool_registry.register(Box::new(GlobTool::new())),
                "todo" => tool_registry.register(Box::new(TodoTool::new())),
                "web_search" => tool_registry.register(Box::new(WebSearchTool::new())),
                "web_fetch" => tool_registry.register(Box::new(WebFetchTool::new())),
                "cron" => tool_registry.register(Box::new(CronTool::new())),
                _ => {
                    eprintln!("Warning: Unknown tool '{}' requested", tool_name);
                }
            }
        }

        // Build agent
        let agent_config = AgentConfig {
            system_prompt: assistant_config.effective_system_prompt(),
            max_context_tokens: 4096,
            max_concurrent_tools: 5,
            temperature: 0.7,
            max_tokens: 2048,
            skills_prompt: None,
        };
        let agent = AgentBuilder::new()
            .config(agent_config)
            .provider(provider)
            .tools(Arc::new(tool_registry))
            .build()?;

        // Set up stdin/stdout for IPC
        let stdin = stdin();
        let stdout = Arc::new(tokio::sync::Mutex::new(stdout()));
        let mut reader = BufReader::new(stdin).lines();

        eprintln!("✅ Assistant ready. Waiting for messages...");

        // IPC loop
        while let Ok(Some(line)) = reader.next_line().await {
            let message: IpcMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("Error parsing IPC message: {}", e);
                    continue;
                }
            };

            match message {
                IpcMessage::ProcessRequest { request_id, message: user_message, context: _ } => {
                    // Process the message
                    let conversation_id = format!("assistant-{}", assistant_id);
                    let incoming = crate::channels::IncomingMessage::new(
                        "parent",
                        &conversation_id,
                        user_message
                    );

                    let response_content = match agent.process_message(incoming).await {
                        Ok(response) => response.content,
                        Err(e) => {
                            format!("Error processing message: {}", e)
                        }
                    };

                    // Send response
                    let response = IpcMessage::ProcessResponse {
                        request_id,
                        response: response_content,
                        success: true,
                        error: None,
                    };

                    let mut stdout = stdout.lock().await;
                    let json = serde_json::to_string(&response).unwrap();
                    if let Err(e) = stdout.write_all(json.as_bytes()).await {
                        eprintln!("Error writing response: {}", e);
                        break;
                    }
                    if let Err(e) = stdout.write_all(b"\n").await {
                        eprintln!("Error writing newline: {}", e);
                        break;
                    }
                    if let Err(e) = stdout.flush().await {
                        eprintln!("Error flushing stdout: {}", e);
                        break;
                    }
                }
                IpcMessage::Ping { request_id } => {
                    let pong = IpcMessage::Pong { request_id };
                    let mut stdout = stdout.lock().await;
                    let json = serde_json::to_string(&pong).unwrap();
                    let _ = stdout.write_all(json.as_bytes()).await;
                    let _ = stdout.write_all(b"\n").await;
                    let _ = stdout.flush().await;
                }
                IpcMessage::Shutdown { reason } => {
                    eprintln!("Shutdown requested: {:?}", reason);
                    break;
                }
                _ => {
                    // Ignore other message types
                }
            }
        }

        eprintln!("👋 Assistant '{}' shutting down", assistant_name);
        Ok(())
    }

    /// Run admin commands for Gateway management
    async fn run_admin_command(&self, command: &AdminCommands) -> Result<()> {
        use crate::client::check_daemon;

        // Check if daemon is running
        let client = match check_daemon().await {
            Ok(client) => client,
            Err(e) => {
                println!("❌ {}", e);
                return Ok(());
            }
        };

        match command {
            AdminCommands::Status => {
                match client.get_status().await {
                    Ok(status) => {
                        println!("🌐 Manta Gateway Status");
                        println!("======================");
                        println!("Agents:      {}/{} busy", status.agents.busy, status.agents.total);
                        println!("Channels:    {}", status.channels);
                        println!("Version:     {}", status.version);
                    }
                    Err(e) => println!("❌ Failed to get status: {}", e),
                }
            }
            AdminCommands::Providers => {
                match client.get_providers().await {
                    Ok(providers) => {
                        println!("🔌 LLM Providers");
                        println!("=================");
                        if providers.is_empty() {
                            println!("No providers configured");
                        } else {
                            println!("{:<20} {:<12} {:<10} {:<15}", "Name", "Type", "Status", "Health");
                            println!("{}", "-".repeat(60));
                            for p in providers {
                                let status = if p.enabled { "✅ enabled" } else { "❌ disabled" };
                                let health = format!("{} ({} failures)", p.health.state, p.health.failures);
                                println!("{:<20} {:<12} {:<10} {:<15}",
                                    truncate(&p.name, 20),
                                    truncate(&p.provider_type, 12),
                                    status,
                                    truncate(&health, 15)
                                );
                            }
                        }
                    }
                    Err(e) => println!("❌ Failed to get providers: {}", e),
                }
            }
            AdminCommands::Models => {
                match client.get_models().await {
                    Ok(models) => {
                        println!("🤖 Model Aliases");
                        println!("=================");
                        if models.aliases.is_empty() {
                            println!("No models configured");
                        } else {
                            for alias in &models.aliases {
                                println!("  • {}", alias);
                            }
                            println!("\nTotal: {} alias(es)", models.aliases.len());
                        }
                    }
                    Err(e) => println!("❌ Failed to get models: {}", e),
                }
            }
            AdminCommands::Default => {
                match client.get_default_model().await {
                    Ok(model) => {
                        println!("📌 Current default model: {}", model.default_model);
                    }
                    Err(e) => println!("❌ Failed to get default model: {}", e),
                }
            }
            AdminCommands::Switch { model } => {
                match client.switch_model(model).await {
                    Ok(result) => {
                        if result.success {
                            println!("✅ {}", result.message);
                        } else {
                            println!("❌ Failed: {}", result.error.unwrap_or_default());
                        }
                    }
                    Err(e) => println!("❌ Failed to switch model: {}", e),
                }
            }
            AdminCommands::Enable { provider } => {
                match client.enable_provider(provider).await {
                    Ok(result) => {
                        if result.success {
                            println!("✅ {}", result.message);
                        } else {
                            println!("❌ Failed: {}", result.error.unwrap_or_default());
                        }
                    }
                    Err(e) => println!("❌ Failed to enable provider: {}", e),
                }
            }
            AdminCommands::Disable { provider } => {
                match client.disable_provider(provider).await {
                    Ok(result) => {
                        if result.success {
                            println!("✅ {}", result.message);
                        } else {
                            println!("❌ Failed: {}", result.error.unwrap_or_default());
                        }
                    }
                    Err(e) => println!("❌ Failed to disable provider: {}", e),
                }
            }
            AdminCommands::Health { provider } => {
                match client.check_provider_health(provider).await {
                    Ok(result) => {
                        println!("🏥 Provider Health Check: {}", provider);
                        println!("========================={}", "=".repeat(provider.len()));
                        println!("Healthy: {}", if result.healthy { "✅ Yes" } else { "❌ No" });
                        println!("Checked: {}", result.checked_at);
                    }
                    Err(e) => println!("❌ Failed to check health: {}", e),
                }
            }
            AdminCommands::Fallback { alias } => {
                match client.get_fallback_chain(alias).await {
                    Ok(chain) => {
                        println!("🔗 Fallback Chain for '{}'", alias);
                        println!("========================={}", "=".repeat(alias.len()));
                        if chain.fallback_chain.is_empty() {
                            println!("No fallback chain configured");
                        } else {
                            for (i, provider) in chain.fallback_chain.iter().enumerate() {
                                println!("  {}. {}", i + 1, provider);
                            }
                        }
                    }
                    Err(e) => println!("❌ Failed to get fallback chain: {}", e),
                }
            }
            AdminCommands::Agents => {
                match client.get_agents().await {
                    Ok(agents) => {
                        println!("🤖 Active Agents");
                        println!("=================");
                        if agents.is_empty() {
                            println!("No agents running");
                        } else {
                            println!("{:<20} {:<10}", "ID", "Status");
                            println!("{}", "-".repeat(35));
                            for agent in &agents {
                                let status = if agent.busy { "🔄 busy" } else { "⏳ idle" };
                                println!("{:<20} {:<10}", truncate(&agent.id, 20), status);
                            }
                            println!("\nTotal: {} agent(s)", agents.len());
                        }
                    }
                    Err(e) => println!("❌ Failed to get agents: {}", e),
                }
            }
            AdminCommands::Send { session_id, message, provider, model } => {
                match client.send_message_with_override(session_id, message, provider.clone(), model.clone()).await {
                    Ok(result) => {
                        if let Some(response) = result.response {
                            println!("🤖 {}", response);
                        } else if result.queued {
                            println!("✅ Message queued (ID: {})", result.message_id);
                        } else {
                            println!("❌ Failed: {}", result.status);
                        }
                    }
                    Err(e) => println!("❌ Failed to send message: {}", e),
                }
            }
        }

        Ok(())
    }
}

/// Truncate a string to a maximum length with ellipsis
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 8), "hello...");
        assert_eq!(truncate("test", 3), "...");
    }

    #[test]
    fn test_status_filter_conversion() {
        assert_eq!(Status::from(StatusFilter::Active), Status::Active);
        assert_eq!(Status::from(StatusFilter::Pending), Status::Pending);
    }
}
