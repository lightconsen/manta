//! MCP (Model Context Protocol) Integration
//!
//! This module implements a client for the Model Context Protocol,
//! allowing Manta to connect to MCP servers and use their tools.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::{error, info};

use super::{Tool, ToolContext, ToolExecutionResult};

/// MCP client for connecting to MCP servers
#[derive(Debug)]
pub struct McpClient {
    /// Server process
    process: Option<Child>,
    /// Request channel
    request_tx: Option<mpsc::UnboundedSender<McpRequest>>,
    /// Server info
    server_info: Option<McpServerInfo>,
    /// Available tools
    tools: Vec<McpToolDefinition>,
    /// Request ID counter
    request_id: std::sync::atomic::AtomicU64,
    /// Response channels
    response_channels: std::sync::Arc<tokio::sync::RwLock<HashMap<u64, mpsc::UnboundedSender<McpResponse>>>>,
}

/// MCP request
#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpRequest {
    jsonrpc: String,
    id: Option<u64>,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// MCP response
#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<McpError>,
}

/// MCP error
#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

/// MCP server information
#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpServerInfo {
    name: String,
    version: String,
}

/// MCP tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpToolDefinition {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// MCP client configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    /// Command to run the MCP server
    pub command: String,
    /// Arguments for the command
    pub args: Vec<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Working directory
    pub working_dir: Option<std::path::PathBuf>,
}

impl McpClient {
    /// Create a new MCP client
    pub fn new() -> Self {
        Self {
            process: None,
            request_tx: None,
            server_info: None,
            tools: Vec::new(),
            request_id: std::sync::atomic::AtomicU64::new(1),
            response_channels: std::sync::Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Connect to an MCP server via stdio
    pub async fn connect_stdio(&mut self, config: McpConfig) -> crate::Result<()> {
        info!("Connecting to MCP server: {}", config.command);

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(dir) = &config.working_dir {
            cmd.current_dir(dir);
        }

        let mut child = cmd.spawn().map_err(|e| {
            crate::error::MantaError::Internal(format!("Failed to spawn MCP server: {}", e))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            crate::error::MantaError::Internal("Failed to get stdin".to_string())
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            crate::error::MantaError::Internal("Failed to get stdout".to_string())
        })?;

        // Set up request channel
        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<McpRequest>();
        self.request_tx = Some(request_tx);

        // Spawn writer task
        let mut stdin_writer = stdin;
        tokio::spawn(async move {
            while let Some(request) = request_rx.recv().await {
                let json = serde_json::to_string(&request).unwrap();
                if let Err(e) = stdin_writer.write_all(json.as_bytes()).await {
                    error!("Failed to write to MCP stdin: {}", e);
                    break;
                }
                if let Err(e) = stdin_writer.write_all(b"\n").await {
                    error!("Failed to write newline: {}", e);
                    break;
                }
                if let Err(e) = stdin_writer.flush().await {
                    error!("Failed to flush stdin: {}", e);
                    break;
                }
            }
        });

        // Spawn reader task
        let response_channels = self.response_channels.clone();
        let stdout_reader = BufReader::new(stdout);
        tokio::spawn(async move {
            let mut lines = stdout_reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(response) = serde_json::from_str::<McpResponse>(&line) {
                    if let Some(id) = response.id {
                        let channels = response_channels.read().await;
                        if let Some(tx) = channels.get(&id) {
                            let _ = tx.send(response);
                        }
                    }
                }
            }
        });

        self.process = Some(child);

        // Initialize connection
        self.initialize().await?;

        info!("Connected to MCP server successfully");
        Ok(())
    }

    /// Initialize the MCP connection
    async fn initialize(&mut self) -> crate::Result<()> {
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(0),
            method: "initialize".to_string(),
            params: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "manta",
                    "version": crate::VERSION,
                },
            })),
        };

        let response = self.send_request(request).await?;

        if let Some(result) = response.result {
            if let Ok(info) = serde_json::from_value::<McpServerInfo>(result) {
                info!("MCP server: {} v{}", info.name, info.version);
                self.server_info = Some(info);
            }
        }

        // List available tools
        self.list_tools().await?;

        Ok(())
    }

    /// Send a request and wait for response
    async fn send_request(&self, request: McpRequest) -> crate::Result<McpResponse> {
        let id = request.id.unwrap_or(0);

        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let mut channels = self.response_channels.write().await;
            channels.insert(id, tx);
        }

        if let Some(ref req_tx) = self.request_tx {
            req_tx.send(request).map_err(|_| {
                crate::error::MantaError::Internal("Request channel closed".to_string())
            })?;
        } else {
            return Err(crate::error::MantaError::Internal("Not connected".to_string()));
        }

        // Wait for response with timeout
        match tokio::time::timeout(tokio::time::Duration::from_secs(30), rx.recv()).await {
            Ok(Some(response)) => {
                let mut channels = self.response_channels.write().await;
                channels.remove(&id);

                if let Some(error) = response.error {
                    return Err(crate::error::MantaError::ExternalService {
                        source: format!("MCP error {}: {}", error.code, error.message),
                        cause: None,
                    });
                }

                Ok(response)
            }
            Ok(None) => Err(crate::error::MantaError::Internal("Response channel closed".to_string())),
            Err(_) => Err(crate::error::MantaError::Internal("Request timeout".to_string())),
        }
    }

    /// List available tools from the MCP server
    async fn list_tools(&mut self) -> crate::Result<()> {
        let id = self.request_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "tools/list".to_string(),
            params: None,
        };

        let response = self.send_request(request).await?;

        if let Some(result) = response.result {
            if let Ok(tools) = serde_json::from_value::<Vec<McpToolDefinition>>(result) {
                info!("Discovered {} MCP tools", tools.len());
                self.tools = tools;
            }
        }

        Ok(())
    }

    /// Call an MCP tool
    async fn call_tool(&self, name: &str, params: serde_json::Value) -> crate::Result<serde_json::Value> {
        let id = self.request_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": name,
                "arguments": params,
            })),
        };

        let response = self.send_request(request).await?;

        response.result.ok_or_else(|| {
            crate::error::MantaError::Internal("No result from tool call".to_string())
        })
    }

    /// Get available tools
    pub fn get_tools(&self) -> Vec<McpToolDefinition> {
        self.tools.clone()
    }

    /// Disconnect from the MCP server
    pub async fn disconnect(&mut self) -> crate::Result<()> {
        info!("Disconnecting from MCP server");

        self.request_tx = None;

        if let Some(mut process) = self.process.take() {
            let _ = process.kill().await;
        }

        Ok(())
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.request_tx.is_some()
    }
}

impl Default for McpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Tool wrapper for MCP tools
#[derive(Debug)]
pub struct McpToolWrapper {
    client: std::sync::Arc<tokio::sync::RwLock<McpClient>>,
    tool_name: String,
    tool_description: String,
    parameters_schema: serde_json::Value,
}

impl McpToolWrapper {
    /// Create a new MCP tool wrapper
    pub fn new(
        client: std::sync::Arc<tokio::sync::RwLock<McpClient>>,
        tool_name: String,
        tool_description: String,
        parameters_schema: serde_json::Value,
    ) -> Self {
        Self {
            client,
            tool_name,
            tool_description,
            parameters_schema,
        }
    }
}

#[async_trait]
impl Tool for McpToolWrapper {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.parameters_schema.clone()
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let client = self.client.read().await;
        let result = client.call_tool(&self.tool_name, args).await?;

        Ok(ToolExecutionResult::success(format!("MCP tool result: {}", result))
            .with_data(result))
    }
}

/// MCP tool for managing MCP connections
#[derive(Debug)]
pub struct McpConnectionTool {
    clients: std::sync::Arc<tokio::sync::RwLock<HashMap<String, McpClient>>>,
}

impl McpConnectionTool {
    /// Create a new MCP connection tool
    pub fn new() -> Self {
        Self {
            clients: std::sync::Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }
}

impl Default for McpConnectionTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for McpConnectionTool {
    fn name(&self) -> &str {
        "mcp"
    }

    fn description(&self) -> &str {
        r#"Connect to and use MCP (Model Context Protocol) servers.

MCP allows connecting to external tool servers that provide additional capabilities.

Actions:
- connect: Connect to an MCP server (stdio transport)
- disconnect: Disconnect from an MCP server
- list: List connected MCP servers
- tools: List available tools from a server

Example:
{"action": "connect", "server_id": "filesystem", "command": "npx", "args": ["-y", "@anthropic/mcp-filesystem"]}"#
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["connect", "disconnect", "list", "tools"],
                    "description": "Action to perform"
                },
                "server_id": {
                    "type": "string",
                    "description": "Unique identifier for the server connection"
                },
                "command": {
                    "type": "string",
                    "description": "Command to run the MCP server (for connect)"
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Arguments for the command (for connect)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("action is required".to_string()))?;

        match action {
            "connect" => {
                let server_id = args["server_id"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation(
                        "server_id is required for connect".to_string()
                    ))?;
                let command = args["command"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation(
                        "command is required for connect".to_string()
                    ))?;
                let args_vec: Vec<String> = args["args"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                let config = McpConfig {
                    command: command.to_string(),
                    args: args_vec,
                    env: HashMap::new(),
                    working_dir: None,
                };

                let mut client = McpClient::new();
                client.connect_stdio(config).await?;

                let mut clients = self.clients.write().await;
                clients.insert(server_id.to_string(), client);

                Ok(ToolExecutionResult::success(format!(
                    "Connected to MCP server: {}", server_id
                )))
            }

            "disconnect" => {
                let server_id = args["server_id"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation(
                        "server_id is required for disconnect".to_string()
                    ))?;

                let mut clients = self.clients.write().await;
                if let Some(mut client) = clients.remove(server_id) {
                    client.disconnect().await?;
                    Ok(ToolExecutionResult::success(format!(
                        "Disconnected from MCP server: {}", server_id
                    )))
                } else {
                    Ok(ToolExecutionResult::error(format!(
                        "MCP server not found: {}", server_id
                    )))
                }
            }

            "list" => {
                let clients = self.clients.read().await;
                let servers: Vec<String> = clients.keys().cloned().collect();

                Ok(ToolExecutionResult::success(format!(
                    "{} MCP servers connected", servers.len()
                )).with_data(json!({"servers": servers})))
            }

            "tools" => {
                let server_id = args["server_id"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation(
                        "server_id is required for tools".to_string()
                    ))?;

                let clients = self.clients.read().await;
                if let Some(client) = clients.get(server_id) {
                    let tools = client.get_tools();
                    Ok(ToolExecutionResult::success(format!(
                        "{} tools available", tools.len()
                    )).with_data(json!({"tools": tools})))
                } else {
                    Ok(ToolExecutionResult::error(format!(
                        "MCP server not found: {}", server_id
                    )))
                }
            }

            _ => Err(crate::error::MantaError::Validation(format!(
                "Unknown action: {}", action
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_request_creation() {
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(1),
            method: "initialize".to_string(),
            params: Some(json!({})),
        };
        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.method, "initialize");
    }

    #[test]
    fn test_mcp_config() {
        let config = McpConfig {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@anthropic/mcp-filesystem".to_string()],
            env: HashMap::new(),
            working_dir: None,
        };
        assert_eq!(config.command, "npx");
        assert_eq!(config.args.len(), 2);
    }

    #[test]
    fn test_mcp_client_default() {
        let client = McpClient::default();
        assert!(!client.is_connected());
        assert!(client.get_tools().is_empty());
    }
}
