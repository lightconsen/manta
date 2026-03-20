//! MCP (Model Context Protocol) CLI commands

use crate::error::{MantaError, Result};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

/// MCP subcommands
#[derive(Debug, Subcommand)]
pub enum McpCommands {
    /// List configured/connected MCP servers
    List,
    /// Connect to an MCP server
    Connect {
        /// Server ID (used as key in config and tool names)
        server_id: String,
        /// Command to run (stdio transport)
        #[arg(short, long)]
        command: Option<String>,
        /// Arguments for the command
        #[arg(short, long)]
        args: Vec<String>,
        /// URL for SSE / streamable-HTTP transport
        #[arg(short, long)]
        url: Option<String>,
        /// Transport type: stdio, sse, streamable_http
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// Timeout in seconds
        #[arg(long, default_value = "30")]
        timeout: u64,
    },
    /// Disconnect from an MCP server
    Disconnect {
        /// Server ID to disconnect
        server_id: String,
    },
    /// List tools available from an MCP server
    Tools {
        /// Server ID (omit to list all servers and their tools)
        server_id: Option<String>,
    },
    /// List resources available from an MCP server
    Resources {
        /// Server ID
        server_id: String,
    },
    /// Call an MCP tool directly
    Call {
        /// Server ID
        server_id: String,
        /// Tool name (without the `mcp__server__` prefix)
        tool: String,
        /// JSON arguments
        #[arg(short, long, default_value = "{}")]
        args: String,
    },
}

pub async fn run_mcp_command(command: &McpCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        McpCommands::List => {
            let url = format!("{}/api/v1/mcp/servers", DAEMON_URL);
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

        McpCommands::Connect {
            server_id,
            command,
            args,
            url,
            transport,
            timeout,
        } => {
            let endpoint = format!("{}/api/v1/mcp/servers/{}/connect", DAEMON_URL, server_id);
            let body = serde_json::json!({
                "command": command,
                "args": args,
                "url": url,
                "transport": transport,
                "timeout": timeout,
            });
            match client.post(&endpoint).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Connected to MCP server '{}'", server_id);
                        println!("{}", text);
                    } else {
                        eprintln!("Failed to connect ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }

        McpCommands::Disconnect { server_id } => {
            let endpoint = format!("{}/api/v1/mcp/servers/{}", DAEMON_URL, server_id);
            match client.delete(&endpoint).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Disconnected MCP server '{}'", server_id);
                    } else {
                        eprintln!("Failed to disconnect ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }

        McpCommands::Tools { server_id } => {
            let endpoint = if let Some(sid) = server_id {
                format!("{}/api/v1/mcp/servers/{}/tools", DAEMON_URL, sid)
            } else {
                format!("{}/api/v1/mcp/servers", DAEMON_URL)
            };
            match client.get(&endpoint).send().await {
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

        McpCommands::Resources { server_id } => {
            let endpoint = format!("{}/api/v1/mcp/servers/{}/resources", DAEMON_URL, server_id);
            match client.get(&endpoint).send().await {
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

        McpCommands::Call { server_id, tool, args } => {
            let endpoint = format!(
                "{}/api/v1/mcp/servers/{}/tools/{}/call",
                DAEMON_URL, server_id, tool
            );
            let parsed_args: serde_json::Value =
                serde_json::from_str(args).unwrap_or(serde_json::json!({}));
            match client.post(&endpoint).json(&parsed_args).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("{}", body);
                    } else {
                        eprintln!("Tool call failed ({}): {}", status, body);
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
