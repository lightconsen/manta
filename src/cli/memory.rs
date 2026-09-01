//! Memory management commands for Syscity
//!
//! Provides CLI access to vector memory over WS (`memory.search` /
//! `memory.add` / `memory.collections`): search and add documents.

use clap::Subcommand;

use crate::cli::ws;
use crate::cli::OutputFormat;
use crate::error::Result;

#[derive(Debug, Subcommand)]
pub enum MemoryCommands {
    /// Search vector memory for relevant documents
    Search {
        /// Query text
        query: String,
        /// Maximum number of results
        #[arg(short, long, default_value = "5")]
        limit: usize,
        /// Collection name
        #[arg(short, long, default_value = "")]
        collection: String,
        /// Output format
        #[arg(short, long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Add a document to vector memory
    Add {
        /// Content to store
        #[arg(short, long)]
        content: String,
        /// Collection name
        #[arg(short, long, default_value = "")]
        collection: String,
        /// Optional metadata (JSON string)
        #[arg(short, long)]
        metadata: Option<String>,
    },
    /// List available memory collections
    Collections,
}

/// Run memory commands
pub async fn run_memory_command(command: &MemoryCommands) -> Result<()> {
    match command {
        MemoryCommands::Search {
            query,
            limit,
            collection,
            format,
        } => {
            let json = ws::call(
                "memory.search",
                serde_json::json!({
                    "query": query,
                    "limit": limit,
                    "collection": collection,
                }),
            )
            .await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&json).unwrap_or_default());
                }
                OutputFormat::Yaml | OutputFormat::Plain => {
                    let results = json.get("results").and_then(|r| r.as_array());
                    if let Some(results) = results {
                        if results.is_empty() {
                            println!("No results found.");
                        } else {
                            println!("Search results for '{}' ({} found):", query, results.len());
                            println!();
                            for (i, r) in results.iter().enumerate() {
                                println!(
                                    "{}. {}",
                                    i + 1,
                                    r.get("content")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("(no content)")
                                );
                                if let Some(score) = r.get("score").and_then(|s| s.as_f64()) {
                                    println!("   Score: {:.4}", score);
                                }
                                println!();
                            }
                        }
                    }
                }
                OutputFormat::Table => {
                    let results = json.get("results").and_then(|r| r.as_array());
                    if let Some(results) = results {
                        if results.is_empty() {
                            println!("No results found.");
                        } else {
                            println!("{:<4} {:<10} Content", "#", "Score");
                            println!("{}", "-".repeat(80));
                            for (i, r) in results.iter().enumerate() {
                                let score = r.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0);
                                let content = r
                                    .get("content")
                                    .and_then(|c| c.as_str())
                                    .unwrap_or("-")
                                    .chars()
                                    .take(60)
                                    .collect::<String>();
                                println!("{:<4} {:<10.4} {}", i + 1, score, content);
                            }
                        }
                    }
                }
            }
            Ok(())
        }

        MemoryCommands::Add { content, collection, metadata } => {
            let json = ws::call(
                "memory.add",
                serde_json::json!({
                    "content": content,
                    "collection": collection,
                    "metadata": metadata.as_ref().and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok()),
                }),
            )
            .await?;
            let doc_id = json
                .get("document_id")
                .and_then(|d| d.as_str())
                .unwrap_or("unknown");
            println!("Document added: {}", doc_id);
            Ok(())
        }

        MemoryCommands::Collections => {
            let json = ws::call("memory.collections", serde_json::json!({})).await?;
            if let Some(collections) = json.get("collections").and_then(|c| c.as_array()) {
                if collections.is_empty() {
                    println!("No collections found.");
                } else {
                    println!("Memory collections:");
                    for c in collections {
                        println!("  - {}", c.as_str().unwrap_or("?"));
                    }
                }
            }
            Ok(())
        }
    }
}
