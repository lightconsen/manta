//! Entity management commands for Manta

use clap::Subcommand;
use crate::core::models::{CreateEntityRequest, Status, UpdateEntityRequest};
use crate::error::Result;

#[derive(Debug, Subcommand)]
pub enum EntityCommands {
    /// List all entities
    List {
        /// Filter by status
        #[arg(short, long, value_enum)]
        status: Option<Status>,
        /// Output format
        #[arg(short, long, value_enum, default_value = "table")]
        format: crate::cli::OutputFormat,
    },
    /// Create a new entity
    Create {
        /// Entity name
        name: String,
        /// Entity type
        #[arg(short, long)]
        entity_type: String,
        /// Initial status
        #[arg(short, long, value_enum, default_value = "active")]
        status: Status,
        /// Metadata as JSON string
        #[arg(short, long)]
        metadata: Option<String>,
    },
    /// Get entity details
    Get {
        /// Entity ID or name
        id: String,
        /// Output format
        #[arg(short, long, value_enum, default_value = "yaml")]
        format: crate::cli::OutputFormat,
    },
    /// Update an entity
    Update {
        /// Entity ID or name
        id: String,
        /// New name
        #[arg(short, long)]
        name: Option<String>,
        /// New status
        #[arg(short, long, value_enum)]
        status: Option<Status>,
        /// New metadata as JSON string
        #[arg(short, long)]
        metadata: Option<String>,
    },
    /// Delete an entity
    Delete {
        /// Entity ID or name
        id: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Search entities
    Search {
        /// Search query
        query: String,
        /// Filter by type
        #[arg(short, long)]
        entity_type: Option<String>,
        /// Output format
        #[arg(short, long, value_enum, default_value = "table")]
        format: crate::cli::OutputFormat,
    },
    /// Export entities
    Export {
        /// Output file path
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
        /// Filter by type
        #[arg(short, long)]
        entity_type: Option<String>,
    },
    /// Import entities
    Import {
        /// Input file path
        path: std::path::PathBuf,
        /// Skip validation
        #[arg(long)]
        no_validate: bool,
    },
}

/// Run entity commands
pub async fn run_entity_command(command: &EntityCommands) -> Result<()> {
    match command {
        EntityCommands::List { status, format } => {
            println!("Listing entities (status={:?}, format={:?})", status, format);
        }
        EntityCommands::Create { name, entity_type, status, metadata } => {
            println!("Creating entity {} (type={}, status={:?}, metadata={:?})",
                name, entity_type, status, metadata);
        }
        EntityCommands::Get { id, format } => {
            println!("Getting entity {} (format={:?})", id, format);
        }
        EntityCommands::Update { id, name, status, metadata } => {
            println!("Updating entity {} (name={:?}, status={:?}, metadata={:?})",
                id, name, status, metadata);
        }
        EntityCommands::Delete { id, force } => {
            println!("Deleting entity {} (force={})", id, force);
        }
        EntityCommands::Search { query, entity_type, format } => {
            println!("Searching for '{}' (type={:?}, format={:?})",
                query, entity_type, format);
        }
        EntityCommands::Export { output, entity_type } => {
            println!("Exporting entities to {:?} (type={:?})", output, entity_type);
        }
        EntityCommands::Import { path, no_validate } => {
            println!("Importing entities from {:?} (validate={})", path, !no_validate);
        }
    }
    Ok(())
}
