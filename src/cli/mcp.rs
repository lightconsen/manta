//! MCP (Model Context Protocol) CLI commands

use crate::error::Result;
use clap::Subcommand;

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
    match command {
        McpCommands::List => {
            println!("Requesting MCP server list from daemon…");
            // In a full implementation this would call the daemon REST API.
            // For now, print a helpful message about the daemon endpoint.
            println!("  GET http://localhost:18080/api/v1/mcp/servers");
            println!("  (start the daemon with `manta start` first)");
        }

        McpCommands::Connect {
            server_id,
            command,
            args,
            url,
            transport,
            timeout,
        } => {
            println!("Connecting to MCP server '{}' via {} transport…", server_id, transport);
            if let Some(cmd) = command {
                println!("  Command : {} {}", cmd, args.join(" "));
            }
            if let Some(u) = url {
                println!("  URL     : {}", u);
            }
            println!("  Timeout : {}s", timeout);
            println!();
            println!("POST http://localhost:18080/api/v1/mcp/servers/{}/connect", server_id);
            println!("(start the daemon with `manta start` first)");
        }

        McpCommands::Disconnect { server_id } => {
            println!("Disconnecting MCP server '{}'…", server_id);
            println!("DELETE http://localhost:18080/api/v1/mcp/servers/{}", server_id);
        }

        McpCommands::Tools { server_id } => {
            if let Some(sid) = server_id {
                println!("GET http://localhost:18080/api/v1/mcp/servers/{}/tools", sid);
            } else {
                println!("GET http://localhost:18080/api/v1/mcp/servers");
            }
        }

        McpCommands::Resources { server_id } => {
            println!("GET http://localhost:18080/api/v1/mcp/servers/{}/resources", server_id);
        }

        McpCommands::Call { server_id, tool, args } => {
            println!("Calling tool '{}' on server '{}' with args: {}", tool, server_id, args);
            println!(
                "POST http://localhost:18080/api/v1/mcp/servers/{}/tools/{}/call",
                server_id, tool
            );
        }
    }

    Ok(())
}
