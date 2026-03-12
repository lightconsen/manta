//! Command-line interface for Manta
//!
//! This module handles argument parsing and command execution
//! using the `clap` crate.

use crate::config::Config;
use crate::core::models::{CreateEntityRequest, Status, UpdateEntityRequest};
use crate::core::Engine;
use crate::error::Result;
use crate::server::{ServerConfig, start_server};
use clap::{Parser, Subcommand, ValueEnum};
use rustyline::{history::History, DefaultEditor, Result as RustyResult};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info, instrument};

/// Manta - A Rust application
#[derive(Debug, Parser)]
#[command(name = "manta")]
#[command(about = "Manta - A Rust application")]
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
    /// Start the server
    Server {
        /// Host to bind to
        #[arg(short, long)]
        host: Option<String>,
        /// Port to listen on
        #[arg(short, long)]
        port: Option<u16>,
    },
    /// Entity management commands
    Entity {
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
    /// Run as an assistant process (internal use)
    AssistantRun {
        /// Configuration file path
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Cron job management
    Cron {
        #[command(subcommand)]
        command: CronCommands,
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
            Commands::Server { host, port } => {
                self.run_server(&config, host.clone(), *port).await
            }
            Commands::Entity { command } => self.run_entity_command(command).await,
            Commands::Config { format } => self.show_config(&config, *format),
            Commands::Health => self.run_health_check(&config).await,
            Commands::Chat { conversation, message } => {
                self.run_chat(&config, conversation.clone(), message.clone()).await
            }
            Commands::AssistantRun { config: assistant_config } => {
                self.run_assistant_process(assistant_config).await
            }
            Commands::Cron { command } => self.run_cron_command(command).await,
        }
    }

    async fn run_server(
        &self,
        config: &Config,
        host: Option<String>,
        port: Option<u16>,
    ) -> Result<()> {
        let host = host.as_ref().unwrap_or(&config.server.host).clone();
        let port = port.unwrap_or(config.server.port);

        info!("Starting server on {}:{}", host, port);
        println!("🚀 Server starting on http://{}:{}", host, port);

        // Create engine
        let engine = Arc::new(Engine::new());

        // Configure and start the server
        let server_config = ServerConfig { host, port };

        println!("Press Ctrl+C to stop");

        // Start the server (it will handle shutdown internally)
        start_server(server_config, engine).await?;

        info!("Shutting down server");
        println!("\n👋 Server stopped");

        Ok(())
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

        let data_dir = dirs::data_dir()
            .ok_or_else(|| crate::error::MantaError::Internal("Could not find data directory".to_string()))?
            .join("manta");
        tokio::fs::create_dir_all(&data_dir).await.ok();
        let jobs_file = data_dir.join("cron_jobs.json");

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

    async fn run_chat(
        &self,
        config: &Config,
        conversation_id: Option<String>,
        single_message: Option<String>,
    ) -> Result<()> {
        use crate::agent::{AgentConfig, AgentBuilder};
        use crate::channels::{ConversationId, IncomingMessage};
        use crate::tools::{ToolRegistry, ShellTool, FileReadTool, FileWriteTool, FileEditTool, GlobTool, TodoTool, MemoryTool, CronTool};
        use tracing::warn;

        println!("🤖 Manta AI Assistant");
        println!("=====================");
        println!();

        // Read environment variables (Claude Code style)
        let base_url = std::env::var("MANTA_BASE_URL").ok();
        let api_key = std::env::var("MANTA_API_KEY").ok();
        let model = std::env::var("MANTA_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
        // Optional: custom API path for non-standard endpoints
        let _api_path = std::env::var("MANTA_API_PATH").ok();

        // Validate required environment variables
        if base_url.is_none() {
            println!("❌ Error: MANTA_BASE_URL environment variable not set.");
            println!();
            println!("Configuration Guide:");
            println!();
            println!("OpenAI-compatible providers (default):");
            println!("  export MANTA_BASE_URL=https://api.openai.com/v1");
            println!("  export MANTA_API_KEY=sk-your-key");
            println!("  export MANTA_MODEL=gpt-4o-mini");
            println!();
            println!("Anthropic/Claude:");
            println!("  export MANTA_IS_ANTHROPIC=true");
            println!("  export MANTA_BASE_URL=https://api.anthropic.com");
            println!("  export MANTA_API_KEY=sk-ant-your-key");
            println!("  export MANTA_MODEL=claude-3-5-sonnet-20241022");
            println!();
            println!("Ollama (local):");
            println!("  export MANTA_BASE_URL=http://localhost:11434/v1");
            println!("  export MANTA_API_KEY=ollama");
            println!("  export MANTA_MODEL=llama3.1");
            println!();
            return Ok(());
        }

        if api_key.is_none() {
            println!("❌ Error: MANTA_API_KEY environment variable not set.");
            println!("   Example: export MANTA_API_KEY=your-api-key");
            println!();
            return Ok(());
        }

        // Check if using Anthropic API format
        let is_anthropic = std::env::var("MANTA_IS_ANTHROPIC")
            .map(|v| v.to_lowercase() == "true" || v == "1")
            .unwrap_or(false);

        // Create provider
        let provider: Arc<dyn crate::providers::Provider> = if is_anthropic {
            use crate::providers::anthropic::AnthropicProvider;
            let base = base_url.unwrap();
            let key = api_key.unwrap();
            println!("✅ Using Anthropic provider at: {}", base);
            println!("✅ Model: {}", model);
            Arc::new(AnthropicProvider::with_base_url(key, base)?.with_model(model))
        } else {
            use crate::providers::openai::OpenAiProvider;
            let base = base_url.unwrap();
            let key = api_key.unwrap();
            println!("✅ Using OpenAI-compatible provider at: {}", base);
            println!("✅ Model: {}", model);
            Arc::new(OpenAiProvider::with_base_url(key, base)?.with_model(model))
        };

        // Create tool registry with default tools
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(ShellTool::new()));
        tool_registry.register(Box::new(FileReadTool::new()));
        tool_registry.register(Box::new(FileWriteTool::new()));
        tool_registry.register(Box::new(FileEditTool::new()));
        tool_registry.register(Box::new(GlobTool::new()));
        tool_registry.register(Box::new(TodoTool::new()));
        tool_registry.register(Box::new(MemoryTool::new()));
        tool_registry.register(Box::new(CronTool::new()));

        // Add MCP tool
        let mcp_tool = crate::tools::McpConnectionTool::new();
        tool_registry.register(Box::new(mcp_tool));
        println!("✅ Loaded MCP connection tool");

        println!("✅ Loaded {} tools", tool_registry.list().len());

        // Build agent
        let agent_config = AgentConfig::default();
        let agent = AgentBuilder::new()
            .config(agent_config)
            .provider(provider)
            .tools(Arc::new(tool_registry))
            .build()?;

        // Generate or use provided conversation ID
        let conversation_id = conversation_id.unwrap_or_else(|| {
            ConversationId::generate().to_string()
        });
        println!("📱 Conversation ID: {}", conversation_id);
        println!();

        // Single message mode
        if let Some(message) = single_message {
            let incoming = IncomingMessage::new("user", &conversation_id, message);
            let response = agent.process_message(incoming).await?;
            println!("🤖 {}", response.content);
            return Ok(());
        }

        // Check if we're running in a TTY (interactive terminal)
        let is_tty = std::io::stdin().is_terminal();

        let agent = Arc::new(agent);

        if is_tty {
            // Terminal UI mode
            Self::run_interactive_terminal(agent, conversation_id).await
        } else {
            // Simple line-based mode for piped input
            Self::run_simple_interactive(agent, conversation_id).await
        }
    }

    /// Run interactive mode with full terminal UI
    async fn run_interactive_terminal(
        agent: Arc<crate::agent::Agent>,
        conversation_id: String,
    ) -> Result<()> {
        use std::io::{self, Write};

        println!("🤖 Manta Terminal Chat - Type 'exit' to quit, 'help' for commands\n");

        // Print initial prompt immediately
        print!("💬 You: > ");
        io::stdout().flush()?;

        // Interactive terminal mode using standard input
        loop {
            // Read input line
            let mut input = String::new();
            match io::stdin().read_line(&mut input) {
                Ok(0) => break, // EOF
                Ok(_) => {}
                Err(e) => {
                    eprintln!("❌ Input error: {}", e);
                    break;
                }
            }

            let input = input.trim();

            if input.is_empty() {
                continue;
            }

            // Handle special commands
            match input.to_lowercase().as_str() {
                "exit" | "quit" => {
                    println!("👋 Goodbye!");
                    break;
                }
                "help" => {
                    println!("📋 Commands: help, exit, tools");
                    continue;
                }
                "tools" => {
                    let tools = agent.get_tools().list();
                    println!("🔧 Tools ({}): {}", tools.len(), tools.join(", "));
                    continue;
                }
                _ => {}
            }

            // Show thinking indicator
            eprint!("🤖 Thinking...");
            io::stderr().flush()?;

            // Process message
            let incoming = crate::channels::IncomingMessage::new("user", &conversation_id, input);

            match agent.process_message(incoming).await {
                Ok(response) => {
                    // Clear thinking indicator
                    eprint!("\r\x1B[2K");
                    // Print response (single line, trimmed)
                    let content = response.content.trim().replace('\n', " ");
                    println!("🤖 {}", content);
                }
                Err(e) => {
                    eprint!("\r\x1B[2K");
                    eprintln!("❌ Error: {}", e);
                }
            }

            // Print prompt for next input
            print!("💬 You: > ");
            io::stdout().flush()?;
        }

        Ok(())
    }

    /// Run simple line-based interactive mode (for non-TTY)
    async fn run_simple_interactive(
        agent: Arc<crate::agent::Agent>,
        conversation_id: String,
    ) -> Result<()> {
        use tokio::io::{AsyncBufReadExt, BufReader, stdin};

        println!("🤖 Manta Terminal Chat - Type 'exit' to quit");

        let stdin = BufReader::new(stdin());
        let mut lines = stdin.lines();

        // Interactive REPL mode
        while let Ok(Some(line)) = lines.next_line().await {
            let input = line.trim();

            if input.is_empty() {
                print!("💬 You: > ");
                continue;
            }

            // Handle special commands
            match input.to_lowercase().as_str() {
                "exit" | "quit" => {
                    println!("👋 Goodbye!");
                    break;
                }
                "help" => {
                    println!("📋 Commands: help, exit, tools");
                    continue;
                }
                "tools" => {
                    let tools = agent.get_tools().list();
                    println!("🔧 Tools ({}): {}", tools.len(), tools.join(", "));
                    continue;
                }
                _ => {}
            }

            // Show thinking indicator
            eprint!("🤖 Thinking...");

            // Process message
            let incoming = crate::channels::IncomingMessage::new("user", &conversation_id, input);

            match agent.process_message(incoming).await {
                Ok(response) => {
                    // Clear thinking and show response (single line)
                    eprint!("\r\x1B[2K");
                    let content = response.content.trim().replace('\n', " ");
                    println!("🤖 {}", content);
                }
                Err(e) => {
                    eprint!("\r\x1B[2K");
                    eprintln!("❌ Error: {}", e);
                }
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
        use std::collections::HashMap;

        // Read environment variables set by parent
        let assistant_id = std::env::var("MANTA_ASSISTANT_ID")
            .unwrap_or_else(|_| "unknown".to_string());
        let assistant_name = std::env::var("MANTA_ASSISTANT_NAME")
            .unwrap_or_else(|_| "Assistant".to_string());
        let assistant_type_str = std::env::var("MANTA_ASSISTANT_TYPE")
            .unwrap_or_else(|_| "specialist".to_string());
        let parent_id = std::env::var("MANTA_PARENT_ASSISTANT_ID").ok();

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
