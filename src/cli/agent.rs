//! Agent personality management commands for Syscity

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::json;

use crate::cli::ws;
use crate::error::{Result, SyscityError};

#[derive(Debug, Subcommand)]
pub enum AgentCommands {
    /// List all agent personalities
    List {
        /// Show all agents including system defaults
        #[arg(short, long)]
        all: bool,
    },
    /// Show current agent configuration
    Show {
        /// Agent name (defaults to current agent)
        name: Option<String>,
    },
    /// Create a new agent personality
    Create {
        /// Agent name
        name: String,
        /// Description of the agent's role
        #[arg(short, long)]
        description: Option<String>,
        /// Copy from existing agent
        #[arg(short, long)]
        copy_from: Option<String>,
    },
    /// Edit agent configuration
    Edit {
        /// Agent name
        name: String,
    },
    /// Delete an agent personality
    Delete {
        /// Agent name
        name: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Switch to a different agent
    Switch {
        /// Agent name
        name: String,
    },
    /// Show agent memory/state
    Memory {
        /// Agent name
        name: Option<String>,
        /// Clear memory
        #[arg(long)]
        clear: bool,
    },
    /// Import an agent from a file
    Import {
        /// Path to agent configuration file
        path: PathBuf,
        /// Agent name (optional, defaults to file name)
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Export an agent to a file
    Export {
        /// Agent name
        name: String,
        /// Output path
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

/// Run agent commands.
pub async fn run_agent_command(command: &AgentCommands) -> Result<()> {
    match command {
        AgentCommands::List { all } => {
            let payload = ws::call("agents.list", json!({ "all": *all })).await?;
            println!("{}", payload);
        }
        AgentCommands::Show { name } => {
            let id = name.as_deref().unwrap_or("default");
            let payload = ws::call("agents.get", json!({ "id": id })).await?;
            println!("{}", payload);
        }
        AgentCommands::Create { name, description, copy_from } => {
            let body = json!({
                "name": name,
                "description": description,
                "copy_from": copy_from,
                "system_prompt": description.clone().unwrap_or_default(),
            });
            match ws::call("agents.create", body).await {
                Ok(payload) => {
                    println!("Agent '{}' created successfully", name);
                    println!("{}", payload);
                }
                Err(e) => {
                    eprintln!("Failed to create agent: {}", e);
                    return Err(e);
                }
            }
        }
        AgentCommands::Edit { name } => {
            edit_agent_interactive(name).await?;
        }
        AgentCommands::Delete { name, force } => {
            if !force {
                println!("Delete agent '{}'? Use --force to confirm.", name);
                return Ok(());
            }
            match ws::call("agents.delete", json!({ "id": name })).await {
                Ok(_) => println!("Agent '{}' deleted", name),
                Err(e) => {
                    eprintln!("Failed to delete agent: {}", e);
                    return Err(e);
                }
            }
        }
        AgentCommands::Switch { name } => {
            match ws::call("agents.default", json!({ "agent_id": name })).await {
                Ok(_) => println!("Switched to agent '{}'", name),
                Err(e) => {
                    eprintln!("Failed to switch agent: {}", e);
                    return Err(e);
                }
            }
        }
        AgentCommands::Memory { name, clear } => {
            let id = name.as_deref().unwrap_or("default");
            if *clear {
                match ws::call("agents.memory.clear", json!({ "agent_id": id })).await {
                    Ok(_) => println!("Memory cleared for agent '{}'", id),
                    Err(e) => {
                        eprintln!("Failed to clear memory: {}", e);
                        return Err(e);
                    }
                }
            } else {
                match ws::call("agents.memory.get", json!({ "agent_id": id })).await {
                    Ok(payload) => {
                        println!(
                            "{}",
                            payload.get("memory").and_then(|m| m.as_str()).unwrap_or("")
                        );
                    }
                    Err(e) => {
                        eprintln!("Failed to get memory: {}", e);
                        return Err(e);
                    }
                }
            }
        }
        AgentCommands::Import { path, name } => {
            let content = tokio::fs::read_to_string(path)
                .await
                .map_err(|e| SyscityError::Internal(format!("Failed to read file: {}", e)))?;
            let body: serde_json::Value = serde_json::from_str(&content)
                .map_err(|e| SyscityError::Internal(format!("Invalid JSON: {}", e)))?;
            let agent_id = name
                .clone()
                .or_else(|| {
                    body.get("agent_id")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .ok_or_else(|| {
                    SyscityError::Internal("Export file has no agent_id; pass --name".into())
                })?;
            let files = body.get("files").cloned().unwrap_or_else(|| json!({}));
            match ws::call("agents.import", json!({ "agent_id": agent_id, "files": files })).await {
                Ok(_) => println!("Agent '{}' imported successfully", agent_id),
                Err(e) => {
                    eprintln!("Failed to import agent: {}", e);
                    return Err(e);
                }
            }
        }
        AgentCommands::Export { name, output } => {
            match ws::call("agents.export", json!({ "agent_id": name })).await {
                Ok(body) => {
                    let text = serde_json::to_string_pretty(&body).unwrap_or_default();
                    if let Some(path) = output {
                        tokio::fs::write(path, &text).await.map_err(|e| {
                            SyscityError::Internal(format!("Failed to write file: {}", e))
                        })?;
                        println!("Agent '{}' exported to {:?}", name, path);
                    } else {
                        println!("{}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to export agent: {}", e);
                    return Err(e);
                }
            }
        }
    }
    Ok(())
}

/// Fetch current agent config, open it in $EDITOR, then PATCH any changes.
async fn edit_agent_interactive(name: &str) -> Result<()> {
    // 1. Fetch current config over WS
    let current = ws::call("agents.get_config", json!({ "agent_id": name })).await?;
    let current_body = serde_json::to_string_pretty(&current).unwrap_or_default();

    // 2. Write to a temp file
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join(format!("syscity-agent-{}.json", name));
    tokio::fs::write(&tmp_path, &current_body)
        .await
        .map_err(|e| SyscityError::Internal(format!("Failed to write temp file: {}", e)))?;

    // 3. Open editor
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    let status = tokio::process::Command::new(&editor)
        .arg(&tmp_path)
        .status()
        .await
        .map_err(|e| {
            SyscityError::Internal(format!("Failed to launch editor '{}': {}", editor, e))
        })?;

    if !status.success() {
        eprintln!("Editor exited with non-zero status");
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Ok(());
    }

    // 4. Read back the edited file
    let new_body = tokio::fs::read_to_string(&tmp_path)
        .await
        .map_err(|e| SyscityError::Internal(format!("Failed to read temp file: {}", e)))?;
    let _ = tokio::fs::remove_file(&tmp_path).await;

    // 5. Skip if nothing changed
    if new_body.trim() == current_body.trim() {
        println!("No changes made to agent '{}'.", name);
        return Ok(());
    }

    // 6. Validate it's still JSON
    let patch_value: serde_json::Value = serde_json::from_str(&new_body)
        .map_err(|e| SyscityError::Internal(format!("Edited content is not valid JSON: {}", e)))?;

    // 7. Push the edited config over WS
    match ws::call("agents.update", json!({ "agent_id": name, "config": patch_value })).await {
        Ok(_) => println!("Agent '{}' updated successfully.", name),
        Err(e) => eprintln!("Failed to update agent: {}", e),
    }

    Ok(())
}
