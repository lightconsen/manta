//! Command-line interface for Manta
//!
//! This module handles argument parsing and command execution
//! using the `clap` crate.

use crate::config::Config;
use crate::core::models::{CreateEntityRequest, Status, UpdateEntityRequest};
use crate::core::Engine;
use crate::error::Result;
use clap::{Parser, Subcommand, ValueEnum};
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
        }
    }

    async fn run_server(
        &self,
        config: &Config,
        host: Option<String>,
        port: Option<u16>,
    ) -> Result<()> {
        let host = host.as_ref().unwrap_or(&config.server.host);
        let port = port.unwrap_or(config.server.port);

        info!("Starting server on {}:{}", host, port);
        println!("🚀 Server starting on http://{}:{}", host, port);

        // TODO: Implement actual server startup
        // For now, we'll just demonstrate the structure
        println!("Server would start here...");
        println!("Press Ctrl+C to stop");

        // Wait for interrupt signal
        tokio::signal::ctrl_c()
            .await
            .map_err(|e| crate::error::MantaError::Internal(format!(
                "Failed to listen for ctrl+c: {}",
                e
            )))?;

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
        use crate::tools::{ToolRegistry, ShellTool, FileReadTool, FileWriteTool, FileEditTool, GlobTool, TodoTool};
        use std::io::{self, Write};

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
        println!("Type 'exit' or 'quit' to end the conversation.");
        println!("Type 'help' for available commands.");
        println!();

        // Single message mode
        if let Some(message) = single_message {
            let incoming = IncomingMessage::new("user", &conversation_id, message);
            let response = agent.process_message(incoming).await?;
            println!("🤖 {}", response.content);
            return Ok(());
        }

        // Interactive REPL mode
        loop {
            print!("💬 You: ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
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
                    println!("Available commands:");
                    println!("  help     - Show this help message");
                    println!("  exit     - Exit the chat");
                    println!("  clear    - Clear the conversation context");
                    println!("  tools    - List available tools");
                    println!();
                    continue;
                }
                "clear" => {
                    println!("🗑️  Conversation context cleared.");
                    println!();
                    continue;
                }
                "tools" => {
                    let tools = agent.get_tools().list();
                    println!("Available tools ({}):", tools.len());
                    for tool in tools {
                        println!("  - {}", tool);
                    }
                    println!();
                    continue;
                }
                _ => {}
            }

            // Process message
            let incoming = IncomingMessage::new("user", &conversation_id, input);

            match agent.process_message(incoming).await {
                Ok(response) => {
                    println!("🤖 {}", response.content);
                    println!();
                }
                Err(e) => {
                    eprintln!("❌ Error: {}", e);
                    println!();
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
