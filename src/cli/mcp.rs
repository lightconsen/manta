//! MCP (Model Context Protocol) CLI commands

use clap::Subcommand;

use crate::error::{Result, SyscityError};

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

/// MCP subcommands
#[derive(Debug, Subcommand)]
pub enum McpCommands {
    /// List configured/connected MCP servers
    List,
    /// Connect to an MCP server (adds it to config.toml)
    Connect {
        /// Server ID (used as key in config and tool names)
        server_id: String,
        /// Command to run (stdio transport)
        #[arg(short, long)]
        command: Option<String>,
        /// Arguments for the command
        #[arg(short, long, allow_hyphen_values = true)]
        args: Vec<String>,
        /// Environment variable in KEY=VALUE form (repeatable)
        #[arg(short = 'e', long, value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// URL for SSE / streamable-HTTP transport
        #[arg(short, long)]
        url: Option<String>,
        /// Transport type: stdio, sse, streamable_http
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// Timeout in seconds
        #[arg(long, default_value = "30")]
        timeout: u64,
        /// Auth type: oauth2 or bearer (remote MCP servers)
        #[arg(long)]
        auth_type: Option<String>,
        /// OAuth client ID
        #[arg(long)]
        client_id: Option<String>,
        /// OAuth authorization endpoint (auto-discovered if omitted)
        #[arg(long)]
        auth_url: Option<String>,
        /// OAuth token endpoint (auto-discovered if omitted)
        #[arg(long)]
        token_url: Option<String>,
        /// OAuth scopes (space-separated)
        #[arg(long)]
        scopes: Option<String>,
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
                    eprintln!("Is the daemon running? Try: syscity start");
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
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
                .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                .collect();
            let endpoint = format!("{}/api/v1/mcp/servers/{}/connect", DAEMON_URL, server_id);
            let body = serde_json::json!({
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

            match client.post(&endpoint).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Connected to MCP server '{}'", server_id);
                        println!("{}", text);
                    } else if status == reqwest::StatusCode::UNAUTHORIZED {
                        handle_mcp_auth(&client, server_id, &text).await?;
                        // Retry once after authorization completes.
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
                                return Err(SyscityError::Internal(e.to_string()));
                            }
                        }
                    } else {
                        eprintln!("Failed to connect ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
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
                    return Err(SyscityError::Internal(e.to_string()));
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
                    return Err(SyscityError::Internal(e.to_string()));
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
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }

        McpCommands::Call { server_id, tool, args } => {
            let endpoint =
                format!("{}/api/v1/mcp/servers/{}/tools/{}/call", DAEMON_URL, server_id, tool);
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
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }
    }

    Ok(())
}

/// Handle an `401 auth_required` connect response: print the authorization URL,
/// open it in the default browser, then poll `auth/status` until the user
/// completes the flow.
async fn handle_mcp_auth(
    client: &reqwest::Client,
    server_id: &str,
    body: &str,
) -> Result<()> {
    let auth_url: Option<String> = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["auth_url"].as_str().map(|s| s.to_string()));

    match auth_url {
        Some(url) => {
            println!();
            println!("🔑 OAuth authorization required for MCP server '{}'", server_id);
            println!("   打开以下链接完成授权：");
            println!("   {}", url);
            println!();
            println!("   正在等待授权完成…（Ctrl+C 取消）");
            open_browser(&url);

            let status_endpoint =
                format!("{}/api/v1/mcp/servers/{}/auth/status", DAEMON_URL, server_id);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
            while std::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                match client.get(&status_endpoint).send().await {
                    Ok(resp) => {
                        if let Ok(json) = resp.json::<serde_json::Value>().await {
                            if json["authorized"].as_bool().unwrap_or(false) {
                                println!("✅ 授权完成，正在连接…");
                                return Ok(());
                            }
                        }
                    }
                    Err(_) => {
                        // Daemon may be restarting; keep polling.
                    }
                }
            }
            eprintln!("⚠️  授权超时（180s）。完成后请重新运行：syscity mcp connect {}", server_id);
            Err(SyscityError::Internal(format!(
                "OAuth authorization for '{}' timed out",
                server_id
            )))
        }
        None => {
            eprintln!("Failed to connect (401): {}", body);
            Err(SyscityError::Internal("MCP connect failed".to_string()))
        }
    }
}

/// Best-effort open of a URL in the default browser.
fn open_browser(url: &str) {
    let command = if cfg!(target_os = "macos") {
        Some("open")
    } else if cfg!(target_os = "linux") {
        Some("xdg-open")
    } else {
        None
    };
    if let Some(command) = command {
        if let Ok(mut child) = std::process::Command::new(command).arg(url).spawn() {
            let _ = child.wait();
        }
    }
}
