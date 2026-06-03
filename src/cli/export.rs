//! Export CLI commands for Syscity
//!
//! Provides export functionality for conversations and memories
//! to various formats (Markdown, JSON, JSONL).

use crate::error::Result;
use crate::export::{ExportFormat, ExportOptions, ExportService};
use crate::memory::UnifiedStore;
use clap::{Subcommand, ValueEnum};
use std::path::PathBuf;

/// Export subcommands
#[derive(Debug, Subcommand)]
pub enum ExportCommands {
    /// Export conversations to a file
    Conversations {
        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
        /// Export format
        #[arg(short, long, value_enum, default_value = "jsonl")]
        format: ExportFormatArg,
        /// Specific conversation ID (exports all if not specified)
        #[arg(short, long)]
        conversation: Option<String>,
        /// Specific user ID (exports all users if not specified)
        #[arg(short, long)]
        user: Option<String>,
        /// Limit number of records
        #[arg(short, long)]
        limit: Option<usize>,
        /// Pretty-print JSON output
        #[arg(long)]
        pretty: bool,
    },
    /// Export memories to a file
    Memories {
        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
        /// Export format
        #[arg(short, long, value_enum, default_value = "jsonl")]
        format: ExportFormatArg,
        /// Specific user ID (exports all users if not specified)
        #[arg(short, long)]
        user: Option<String>,
        /// Filter by memory type
        #[arg(short = 't', long)]
        memory_type: Option<String>,
        /// Include embeddings in export (significantly increases file size)
        #[arg(long)]
        embeddings: bool,
        /// Limit number of records
        #[arg(short, long)]
        limit: Option<usize>,
        /// Pretty-print JSON output
        #[arg(long)]
        pretty: bool,
    },
    /// Export everything (conversations + memories)
    All {
        /// Output directory path
        #[arg(short, long)]
        output: PathBuf,
        /// Export format
        #[arg(short, long, value_enum, default_value = "jsonl")]
        format: ExportFormatArg,
        /// Specific user ID (exports all users if not specified)
        #[arg(short, long)]
        user: Option<String>,
        /// Include embeddings in export
        #[arg(long)]
        embeddings: bool,
        /// Limit number of records per type
        #[arg(short, long)]
        limit: Option<usize>,
        /// Pretty-print JSON output
        #[arg(long)]
        pretty: bool,
    },
}

/// Export format argument type
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ExportFormatArg {
    /// Markdown format (human-readable)
    Markdown,
    /// JSON format (structured)
    Json,
    /// JSON Lines format (streaming-friendly)
    Jsonl,
}

impl From<ExportFormatArg> for ExportFormat {
    fn from(arg: ExportFormatArg) -> Self {
        match arg {
            ExportFormatArg::Markdown => ExportFormat::Markdown,
            ExportFormatArg::Json => ExportFormat::Json,
            ExportFormatArg::Jsonl => ExportFormat::Jsonl,
        }
    }
}

/// Run export commands
pub async fn run_export_command(command: &ExportCommands) -> Result<()> {
    // Initialize the database store
    let db_path = crate::dirs::data_dir().join("syscity.db");
    let database_url = format!("sqlite://{}", db_path.display());

    let store = UnifiedStore::new(&database_url).await?;
    let service = ExportService::new(store);

    match command {
        ExportCommands::Conversations {
            output,
            format,
            conversation,
            user,
            limit,
            pretty,
        } => {
            let options = if *pretty {
                ExportOptions::new().format((*format).into()).pretty()
            } else {
                ExportOptions::new().format((*format).into())
            };

            let options = if let Some(conv) = conversation {
                options.for_conversation(conv.clone())
            } else {
                options
            };

            let options = if let Some(u) = user {
                options.for_user(u.clone())
            } else {
                options
            };

            let options = if let Some(l) = limit {
                options.limit(*l)
            } else {
                options
            };

            println!("Exporting conversations to {}...", output.display());
            let stats = service.export_conversations(output, &options).await?;

            println!("\n✓ Export complete!");
            println!("  Conversations: {}", stats.conversation_count);
            println!("  Messages: {}", stats.message_count);
            println!("  File size: {} bytes", stats.bytes_written);
        }

        ExportCommands::Memories {
            output,
            format,
            user,
            memory_type,
            embeddings,
            limit,
            pretty,
        } => {
            let options = if *pretty {
                ExportOptions::new().format((*format).into()).pretty()
            } else {
                ExportOptions::new().format((*format).into())
            };

            let options = if *embeddings {
                options.with_embeddings()
            } else {
                options
            };

            let options = if let Some(u) = user {
                options.for_user(u.clone())
            } else {
                options
            };

            let options = if let Some(t) = memory_type {
                options.of_type(t.clone())
            } else {
                options
            };

            let options = if let Some(l) = limit {
                options.limit(*l)
            } else {
                options
            };

            println!("Exporting memories to {}...", output.display());
            let stats = service.export_memories(output, &options).await?;

            println!("\n✓ Export complete!");
            println!("  Memories: {}", stats.memory_count);
            println!("  File size: {} bytes", stats.bytes_written);
        }

        ExportCommands::All {
            output,
            format,
            user,
            embeddings,
            limit,
            pretty,
        } => {
            let options = if *pretty {
                ExportOptions::new().format((*format).into()).pretty()
            } else {
                ExportOptions::new().format((*format).into())
            };

            let options = if *embeddings {
                options.with_embeddings()
            } else {
                options
            };

            let options = if let Some(u) = user {
                options.for_user(u.clone())
            } else {
                options
            };

            let options = if let Some(l) = limit {
                options.limit(*l)
            } else {
                options
            };

            println!("Running full export to {}...", output.display());
            let stats = service.export_all(output, &options).await?;

            let ext = ExportFormat::from(*format).extension();
            println!("\n✓ Export complete!");
            println!("  Files created:");
            println!("    - conversations.{}", ext);
            println!("    - memories.{}", ext);
            println!("    - export.json (metadata)");
            println!("\n  Statistics:");
            println!("    Conversations: {}", stats.conversation_count);
            println!("    Messages: {}", stats.message_count);
            println!("    Memories: {}", stats.memory_count);
            println!("    Total size: {} bytes", stats.bytes_written);
        }
    }

    Ok(())
}
