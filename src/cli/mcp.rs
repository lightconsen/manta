//! MCP server management commands for Syscity

use clap::Subcommand;
use serde_json::json;

use crate::cli::ws;
use crate::error::Result;

#[derive(Debug, Subcommand)]
pub enum McpCommands {
    /// List all MCP servers
    List,
    /// Connect to an MCP server
    Connect {
        /// Server ID
        server_id: String,
        /// Command to run (for stdio transport)
        command: String,
        /// Arguments to the command
        #[arg(short, long)]
        args: Vec<String>,
        /// Environment variables (KEY=VALUE)
        #[arg(short, long)]
        env: Vec<String>,
        /// URL (for HTTP transport)
        #[arg(short, long)]
        url: Option<String>,
        /// Transport type
        #[arg(short, long, default_value = "stdio")]
        transport: String,
        /// Timeout in seconds
        #[arg(short, long, default_value = "30")]
        timeout: u64,
        /// OAuth auth type
        #[arg(short, long)]
        auth_type: Option<String>,
        /// OAuth client ID
        #[arg(long)]
        client_id: Option<String>,
        /// OAuth auth URL
        #[arg(long)]
        auth_url: Option<String>,
        /// OAuth token URL
        #[arg(long)]
        token_url: Option<String>,
        /// OAuth scopes
        #[arg(long)]
        scopes: Option<Vec<String>>,
    },
    /// Disconnect from an MCP server
    Disconnect {
        /// Server ID
        server_id: String,
    },
    /// List tools on a connected server (or all servers)
    Tools {
        /// Server ID (omit for all)
        server_id: Option<String>,
    },
    /// List resources on a connected server
    Resources {
        /// Server ID
        server_id: String,
    },
    /// Call a tool on a connected server
    Call {
        /// Server ID
        server_id: String,
        /// Tool name
        tool: String,
        /// Tool arguments as JSON
        #[arg(short, long)]
        args: String,
    },
}

/// Run MCP commands (over WebSocket).
pub async fn run_mcp_command(command: &McpCommands) -> Result<()> {
    match command {
        McpCommands::List => {
            let payload = ws::call("mcp.list", json!({})).await?;
            println!("{}", payload);
        }

        McpCommands::Connect {
            server_id,
            command,
            args,
            env,
            url,
            transport,
            timeout,
            auth_type,
            client_id,
            auth_url,
            token_url,
            scopes,
        } => {
            let env_map: std::collections::HashMap<String, String> = env
                .iter()
                .filter_map(|kv| {
                    kv.split_once('=')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                })
                .collect();
            let body = json!({
                "id": server_id,
                "command": command,
                "args": args,
                "env": env_map,
                "url": url,
                "transport": transport,
                "timeout_secs": timeout,
                "auth_type": auth_type,
                "client_id": client_id,
                "auth_url": auth_url,
                "token_url": token_url,
                "scopes": scopes,
            });
            match ws::call("mcp.connect", body).await {
                Ok(payload) => println!("Connected MCP server '{}' — {}", server_id, payload),
                Err(e) => {
                    eprintln!("Failed to connect: {}", e);
                    return Err(e);
                }
            }
        }

        McpCommands::Disconnect { server_id } => {
            match ws::call("mcp.disconnect", json!({ "server_id": server_id })).await {
                Ok(_) => println!("Disconnected MCP server '{}'", server_id),
                Err(e) => {
                    eprintln!("Failed to disconnect: {}", e);
                    return Err(e);
                }
            }
        }

        McpCommands::Tools { server_id } => {
            let payload = match server_id {
                Some(sid) => ws::call("mcp.tools", json!({ "server_id": sid })).await?,
                None => ws::call("mcp.list", json!({})).await?,
            };
            println!("{}", payload);
        }

        McpCommands::Resources { server_id } => {
            let payload = ws::call("mcp.resources", json!({ "server_id": server_id })).await?;
            println!("{}", payload);
        }

        McpCommands::Call { server_id, tool, args } => {
            let parsed_args: serde_json::Value =
                serde_json::from_str(args).unwrap_or(serde_json::json!({}));
            let body = json!({
                "server_id": server_id,
                "tool": tool,
                "args": parsed_args,
            });
            match ws::call("mcp.call_tool", body).await {
                Ok(payload) => println!("{}", payload),
                Err(e) => {
                    eprintln!("Tool call failed: {}", e);
                    return Err(e);
                }
            }
        }
    }

    Ok(())
}
