//! Entity management commands for Manta

use crate::core::models::Status;
use crate::error::{MantaError, Result};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

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
    let client = reqwest::Client::new();

    match command {
        EntityCommands::List { status, format } => {
            let mut params = Vec::new();
            if let Some(s) = status {
                params.push(format!("status={:?}", s).to_lowercase());
            }
            let fmt_str = match format {
                crate::cli::OutputFormat::Table => "table",
                crate::cli::OutputFormat::Json => "json",
                crate::cli::OutputFormat::Yaml => "yaml",
                crate::cli::OutputFormat::Plain => "plain",
            };
            params.push(format!("format={}", fmt_str));
            let mut url = format!("{}/api/v1/entities", DAEMON_URL);
            if !params.is_empty() {
                url.push('?');
                url.push_str(&params.join("&"));
            }
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    println!("{}", body);
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon at {}: {}", DAEMON_URL, e);
                    eprintln!("Is the daemon running? Try: manta start");
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        EntityCommands::Create { name, entity_type, status, metadata } => {
            let url = format!("{}/api/v1/entities", DAEMON_URL);
            let meta_value: serde_json::Value = metadata
                .as_deref()
                .and_then(|m| serde_json::from_str(m).ok())
                .unwrap_or(serde_json::Value::Null);
            let body = serde_json::json!({
                "name": name,
                "entity_type": entity_type,
                "status": format!("{:?}", status).to_lowercase(),
                "metadata": meta_value,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status_code = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status_code.is_success() {
                        println!("Entity '{}' created", name);
                        println!("{}", text);
                    } else {
                        eprintln!("Failed to create entity ({}): {}", status_code, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        EntityCommands::Get { id, format } => {
            let fmt_str = match format {
                crate::cli::OutputFormat::Table => "table",
                crate::cli::OutputFormat::Json => "json",
                crate::cli::OutputFormat::Yaml => "yaml",
                crate::cli::OutputFormat::Plain => "plain",
            };
            let url = format!("{}/api/v1/entities/{}?format={}", DAEMON_URL, id, fmt_str);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    println!("{}", body);
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        EntityCommands::Update { id, name, status, metadata } => {
            let url = format!("{}/api/v1/entities/{}", DAEMON_URL, id);
            let meta_value: Option<serde_json::Value> = metadata
                .as_deref()
                .and_then(|m| serde_json::from_str(m).ok());
            let body = serde_json::json!({
                "name": name,
                "status": status.as_ref().map(|s| format!("{:?}", s).to_lowercase()),
                "metadata": meta_value,
            });
            match client.put(&url).json(&body).send().await {
                Ok(resp) => {
                    let status_code = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status_code.is_success() {
                        println!("Entity '{}' updated", id);
                        println!("{}", text);
                    } else {
                        eprintln!("Failed to update entity ({}): {}", status_code, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        EntityCommands::Delete { id, force } => {
            if !force {
                println!("Delete entity '{}'? Use --force to confirm.", id);
                return Ok(());
            }
            let url = format!("{}/api/v1/entities/{}", DAEMON_URL, id);
            match client.delete(&url).send().await {
                Ok(resp) => {
                    let status_code = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status_code.is_success() {
                        println!("Entity '{}' deleted", id);
                    } else {
                        eprintln!("Failed to delete entity ({}): {}", status_code, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        EntityCommands::Search { query, entity_type, format } => {
            let fmt_str = match format {
                crate::cli::OutputFormat::Table => "table",
                crate::cli::OutputFormat::Json => "json",
                crate::cli::OutputFormat::Yaml => "yaml",
                crate::cli::OutputFormat::Plain => "plain",
            };
            let url = format!("{}/api/v1/entities/search?format={}", DAEMON_URL, fmt_str);
            let body = serde_json::json!({
                "query": query,
                "entity_type": entity_type,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    println!("{}", body);
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        EntityCommands::Export { output, entity_type } => {
            let mut url = format!("{}/api/v1/entities/export", DAEMON_URL);
            if let Some(et) = entity_type {
                url = format!("{}?entity_type={}", url, et);
            }
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    if let Some(path) = output {
                        tokio::fs::write(path, &body).await.map_err(|e| {
                            MantaError::Internal(format!("Failed to write export file: {}", e))
                        })?;
                        println!("Entities exported to {:?}", path);
                    } else {
                        println!("{}", body);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        EntityCommands::Import { path, no_validate } => {
            let content = tokio::fs::read_to_string(path).await.map_err(|e| {
                MantaError::Internal(format!("Failed to read file {:?}: {}", path, e))
            })?;
            let url = format!("{}/api/v1/entities/import", DAEMON_URL);
            let body = serde_json::json!({
                "data": content,
                "validate": !no_validate,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status_code = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status_code.is_success() {
                        println!("Entities imported from {:?}", path);
                        println!("{}", text);
                    } else {
                        eprintln!("Failed to import entities ({}): {}", status_code, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
    }
    Ok(())
}
