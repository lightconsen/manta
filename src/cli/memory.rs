//! Memory management commands for Manta
//!
//! Provides CLI access to vector memory: search and add documents.

use crate::cli::OutputFormat;
use crate::error::{MantaError, Result};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

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
    let client = reqwest::Client::new();

    match command {
        MemoryCommands::Search {
            query,
            limit,
            collection,
            format,
        } => {
            let url = format!("{}/api/v1/memory/search", DAEMON_URL);
            let body = serde_json::json!({
                "query": query,
                "limit": limit,
                "collection": collection,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let json: serde_json::Value = resp.json().await.unwrap_or_default();
                    if !status.is_success() {
                        eprintln!("Search failed ({}): {}", status, json);
                        return Ok(());
                    }
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
                                    println!(
                                        "Search results for '{}' ({} found):",
                                        query,
                                        results.len()
                                    );
                                    println!();
                                    for (i, r) in results.iter().enumerate() {
                                        println!(
                                            "{}. {}",
                                            i + 1,
                                            r.get("content")
                                                .and_then(|c| c.as_str())
                                                .unwrap_or("(no content)")
                                        );
                                        if let Some(score) = r.get("score").and_then(|s| s.as_f64())
                                        {
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
                                    println!("{:<4} {:<10} {}", "#", "Score", "Content");
                                    println!("{}", "-".repeat(80));
                                    for (i, r) in results.iter().enumerate() {
                                        let score =
                                            r.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0);
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
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }

        MemoryCommands::Add { content, collection, metadata } => {
            let url = format!("{}/api/v1/memory/add", DAEMON_URL);
            let body = serde_json::json!({
                "content": content,
                "collection": collection,
                "metadata": metadata.as_ref().and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok()),
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let body: serde_json::Value = resp.json().await.unwrap_or_default();
                        let doc_id = body
                            .get("document_id")
                            .and_then(|d| d.as_str())
                            .unwrap_or("unknown");
                        println!("Document added: {}", doc_id);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to add memory: {}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }

        MemoryCommands::Collections => {
            let url = format!("{}/api/v1/memory/collections", DAEMON_URL);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    if let Some(collections) = body.get("collections").and_then(|c| c.as_array()) {
                        if collections.is_empty() {
                            println!("No collections found.");
                        } else {
                            println!("Memory collections:");
                            for c in collections {
                                println!("  - {}", c.as_str().unwrap_or("?"));
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
    }
}
