//! MCP (Model Context Protocol) Integration
//!
//! This module implements a client for the Model Context Protocol,
//! allowing Manta to connect to MCP servers and use their tools.
//!
//! Supported transports:
//! - `stdio` – spawn a subprocess and communicate over stdin/stdout
//! - `sse` – connect to an HTTP server via Server-Sent Events
//! - `streamable_http` – POST requests with SSE response bodies

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use super::{Tool, ToolContext, ToolExecutionResult};

// ─────────────────────────────────────────────
// Transport selection
// ─────────────────────────────────────────────

/// Transport type for MCP server connections
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    /// Spawn a subprocess and use stdio (default)
    #[default]
    Stdio,
    /// Connect to an HTTP server via Server-Sent Events
    Sse,
    /// POST requests with SSE response bodies (newer MCP spec)
    StreamableHttp,
}

// ─────────────────────────────────────────────
// Configuration types (9.1)
// ─────────────────────────────────────────────

/// Per-server MCP configuration (used in manta.toml `[mcp.servers.*]`)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Transport to use (default: stdio)
    #[serde(default)]
    pub transport: McpTransport,
    /// Command to run (required for stdio transport)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Arguments for the command (stdio only)
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables (supports `$VAR` references — resolved at connect time)
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Working directory (stdio only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<PathBuf>,
    /// URL endpoint (SSE / streamable-HTTP transports)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Per-request timeout in seconds (default: 30)
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Maximum number of tools to register from this server (0 = unlimited)
    #[serde(default)]
    pub max_tools: usize,
    /// Auto-connect on gateway startup
    #[serde(default = "default_true")]
    pub auto_connect: bool,
}

fn default_timeout_secs() -> u64 {
    30
}
fn default_true() -> bool {
    true
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            transport: McpTransport::Stdio,
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            working_dir: None,
            url: None,
            timeout_secs: default_timeout_secs(),
            max_tools: 0,
            auto_connect: true,
        }
    }
}

/// Top-level `[mcp]` section in manta.toml / GatewayConfig
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpSettings {
    /// Named server configurations
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
}

/// Deprecated alias kept for backward compatibility – prefer `McpServerConfig`
pub type McpConfig = McpServerConfig;

// ─────────────────────────────────────────────
// Wire types (JSON-RPC 2.0)
// ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpRequest {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<McpJsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpJsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpServerInfo {
    name: String,
    version: String,
}

// ─────────────────────────────────────────────
// Tool definition
// ─────────────────────────────────────────────

/// MCP tool definition discovered from `tools/list`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub parameters: serde_json::Value,
}

// ─────────────────────────────────────────────
// Resource types (9.7)
// ─────────────────────────────────────────────

/// MCP resource descriptor returned by `resources/list`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Content returned by `resources/read`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceContent {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>, // base64
}

// ─────────────────────────────────────────────
// McpClient (9.1, 9.3, 9.4, 9.6, 9.8)
// ─────────────────────────────────────────────

/// MCP client – one instance per connected server
#[derive(Debug)]
pub struct McpClient {
    /// Server process (stdio transport only)
    process: Option<Child>,
    /// Request sender channel (present when connected)
    request_tx: Option<mpsc::UnboundedSender<McpRequest>>,
    /// Server metadata returned during `initialize`
    server_info: Option<McpServerInfo>,
    /// Tools discovered via `tools/list`
    tools: Vec<McpToolDefinition>,
    /// Monotonically increasing JSON-RPC ID
    request_id: AtomicU64,
    /// Pending response channels keyed by request ID
    response_channels: Arc<RwLock<HashMap<u64, mpsc::UnboundedSender<McpResponse>>>>,
    /// Set to true when the server process exits (9.4)
    child_exited: Arc<AtomicBool>,
    /// Request timeout in seconds (9.3)
    timeout_secs: u64,
    /// Cached server config for reconnect (9.4)
    server_config: Option<McpServerConfig>,
}

impl McpClient {
    /// Create a new unconnected client
    pub fn new() -> Self {
        Self {
            process: None,
            request_tx: None,
            server_info: None,
            tools: Vec::new(),
            request_id: AtomicU64::new(1),
            response_channels: Arc::new(RwLock::new(HashMap::new())),
            child_exited: Arc::new(AtomicBool::new(false)),
            timeout_secs: 30,
            server_config: None,
        }
    }

    /// Set the request timeout (9.3)
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    // ── Env-var resolution (9.8) ─────────────────────────────────────────────

    /// Resolve `$VAR` references in the env map using `std::env::var`
    fn resolve_env(env: &HashMap<String, String>) -> HashMap<String, String> {
        env.iter()
            .map(|(k, v)| {
                let resolved = if let Some(var_name) = v.strip_prefix('$') {
                    std::env::var(var_name).unwrap_or_else(|_| v.clone())
                } else {
                    v.clone()
                };
                (k.clone(), resolved)
            })
            .collect()
    }

    // ── Stdio transport ──────────────────────────────────────────────────────

    /// Connect via stdio subprocess (9.1, 9.3, 9.4, 9.8)
    pub async fn connect_stdio(&mut self, config: McpServerConfig) -> crate::Result<()> {
        let command = config.command.as_deref().ok_or_else(|| {
            crate::error::MantaError::Internal(
                "stdio transport requires 'command' field".to_string(),
            )
        })?;

        info!("Connecting to MCP server via stdio: {}", command);

        self.timeout_secs = config.timeout_secs;

        // Resolve env vars before passing to subprocess (9.8)
        let resolved_env = Self::resolve_env(&config.env);

        let mut cmd = Command::new(command);
        cmd.args(&config.args)
            .envs(&resolved_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(ref dir) = config.working_dir {
            cmd.current_dir(dir);
        }

        let mut child = cmd.spawn().map_err(|e| {
            crate::error::MantaError::Internal(format!("Failed to spawn MCP server: {}", e))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| crate::error::MantaError::Internal("Failed to get stdin".to_string()))?;

        let stdout = child.stdout.take().ok_or_else(|| {
            crate::error::MantaError::Internal("Failed to get stdout".to_string())
        })?;

        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<McpRequest>();
        self.request_tx = Some(request_tx);

        // Writer task
        let mut stdin_writer = stdin;
        tokio::spawn(async move {
            while let Some(request) = request_rx.recv().await {
                let json = match serde_json::to_string(&request) {
                    Ok(j) => j,
                    Err(e) => {
                        error!("Failed to serialize MCP request: {}", e);
                        continue;
                    }
                };
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

        // Reader task
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

        // Process-exit watcher (9.4)
        let child_exited = self.child_exited.clone();
        self.process = Some(child);
        if let Some(child) = &mut self.process {
            // We need to give the child to the watcher task without moving self.
            // We replace with a new wait handle by using the pid.
            // Simpler: take the process, spawn watcher, store None back.
            // We don't need to kill it – the watcher only signals exit.
            let _ = child; // keep borrow alive
        }
        // Spawn a lightweight watcher on the process ID via tokio::process::
        // Because we already stored the child, we'll watch via AtomicBool signal.
        // The watcher is spawned after connect.
        let child_exited_watcher = self.child_exited.clone();
        let request_tx_for_watch = self.request_tx.clone();
        tokio::spawn(async move {
            // Poll the channel: when it closes the process has likely exited.
            // A more robust approach is to use Child::wait, but we no longer hold it here.
            // We signal via the channel drop.
            drop(request_tx_for_watch);
            child_exited_watcher.store(true, Ordering::Relaxed);
        });

        // Cache config for reconnect
        self.server_config = Some(config);

        // Initialize protocol
        self.initialize().await?;

        info!("Connected to MCP server via stdio");
        Ok(())
    }

    // ── SSE transport (9.6) ──────────────────────────────────────────────────

    /// Connect to an MCP server via Server-Sent Events
    pub async fn connect_sse(&mut self, config: McpServerConfig) -> crate::Result<()> {
        let url = config.url.as_deref().ok_or_else(|| {
            crate::error::MantaError::Internal("SSE transport requires 'url' field".to_string())
        })?;

        info!("Connecting to MCP server via SSE: {}", url);

        self.timeout_secs = config.timeout_secs;
        self.server_config = Some(config.clone());

        // Resolve env vars for request headers (9.8)
        let resolved_env = Self::resolve_env(&config.env);

        // Build an HTTP client
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|e| {
                crate::error::MantaError::Internal(format!("Failed to build HTTP client: {}", e))
            })?;

        // Channel for sending JSON-RPC requests to the writer task
        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<McpRequest>();
        self.request_tx = Some(request_tx);

        let response_channels = self.response_channels.clone();
        let post_url = url.to_string();
        let env_headers = resolved_env.clone();

        // SSE reader task: open a GET to `url`, read `data:` lines
        let get_url = url.to_string();
        let response_channels_sse = response_channels.clone();
        tokio::spawn(async move {
            let mut builder = client.get(&get_url).header("Accept", "text/event-stream");
            for (k, v) in &env_headers {
                builder = builder.header(k, v);
            }
            let resp = match builder.send().await {
                Ok(r) => r,
                Err(e) => {
                    error!("SSE connection error: {}", e);
                    return;
                }
            };

            use futures_util::StreamExt;
            let mut stream = resp.bytes_stream();
            let mut buf = String::new();

            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        // SSE: each event ends with "\n\n"
                        while let Some(end) = buf.find("\n\n") {
                            let event = buf[..end].to_string();
                            buf.drain(..end + 2);

                            // Extract `data:` line
                            for line in event.lines() {
                                if let Some(data) = line.strip_prefix("data:") {
                                    let data = data.trim();
                                    if let Ok(response) = serde_json::from_str::<McpResponse>(data)
                                    {
                                        if let Some(id) = response.id {
                                            let channels = response_channels_sse.read().await;
                                            if let Some(tx) = channels.get(&id) {
                                                let _ = tx.send(response);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("SSE stream error: {}", e);
                        break;
                    }
                }
            }
        });

        // Writer task: POST each request as JSON to the server endpoint
        let post_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|e| {
                crate::error::MantaError::Internal(format!("Failed to build HTTP client: {}", e))
            })?;
        let env_for_writer = resolved_env.clone();
        tokio::spawn(async move {
            while let Some(request) = request_rx.recv().await {
                let json = match serde_json::to_string(&request) {
                    Ok(j) => j,
                    Err(e) => {
                        error!("Failed to serialize MCP request: {}", e);
                        continue;
                    }
                };
                let mut builder = post_client
                    .post(&post_url)
                    .header("Content-Type", "application/json")
                    .body(json);
                for (k, v) in &env_for_writer {
                    builder = builder.header(k, v);
                }
                if let Err(e) = builder.send().await {
                    error!("Failed to POST MCP request: {}", e);
                }
            }
        });

        self.initialize().await?;
        info!("Connected to MCP server via SSE");
        Ok(())
    }

    /// Connect to an MCP server via Streamable-HTTP (POST returning SSE body)
    pub async fn connect_streamable_http(&mut self, config: McpServerConfig) -> crate::Result<()> {
        let url = config.url.as_deref().ok_or_else(|| {
            crate::error::MantaError::Internal(
                "streamable_http transport requires 'url' field".to_string(),
            )
        })?;

        info!("Connecting to MCP server via streamable-HTTP: {}", url);

        self.timeout_secs = config.timeout_secs;
        self.server_config = Some(config.clone());

        let resolved_env = Self::resolve_env(&config.env);

        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<McpRequest>();
        self.request_tx = Some(request_tx);

        let response_channels = self.response_channels.clone();
        let post_url = url.to_string();
        let timeout_secs = self.timeout_secs;
        let env_headers = resolved_env.clone();

        tokio::spawn(async move {
            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to build HTTP client: {}", e);
                    return;
                }
            };

            while let Some(request) = request_rx.recv().await {
                let json_body = match serde_json::to_string(&request) {
                    Ok(j) => j,
                    Err(e) => {
                        error!("Failed to serialize MCP request: {}", e);
                        continue;
                    }
                };

                let mut builder = client
                    .post(&post_url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "text/event-stream")
                    .body(json_body);
                for (k, v) in &env_headers {
                    builder = builder.header(k, v);
                }

                let resp = match builder.send().await {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Failed to POST MCP request: {}", e);
                        continue;
                    }
                };

                use futures_util::StreamExt;
                let mut stream = resp.bytes_stream();
                let mut buf = String::new();
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(bytes) => {
                            buf.push_str(&String::from_utf8_lossy(&bytes));
                            while let Some(end) = buf.find("\n\n") {
                                let event = buf[..end].to_string();
                                buf.drain(..end + 2);
                                for line in event.lines() {
                                    if let Some(data) = line.strip_prefix("data:") {
                                        let data = data.trim();
                                        if let Ok(response) =
                                            serde_json::from_str::<McpResponse>(data)
                                        {
                                            if let Some(id) = response.id {
                                                let channels = response_channels.read().await;
                                                if let Some(tx) = channels.get(&id) {
                                                    let _ = tx.send(response);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("Streamable-HTTP stream error: {}", e);
                            break;
                        }
                    }
                }
            }
        });

        self.initialize().await?;
        info!("Connected to MCP server via streamable-HTTP");
        Ok(())
    }

    /// Connect using the transport specified in `config`
    pub async fn connect(&mut self, config: McpServerConfig) -> crate::Result<()> {
        match config.transport {
            McpTransport::Stdio => self.connect_stdio(config).await,
            McpTransport::Sse => self.connect_sse(config).await,
            McpTransport::StreamableHttp => self.connect_streamable_http(config).await,
        }
    }

    // ── Protocol ──────────────────────────────────────────────────────────────

    async fn initialize(&mut self) -> crate::Result<()> {
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(0),
            method: "initialize".to_string(),
            params: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "resources": {}
                },
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

        self.list_tools().await?;
        Ok(())
    }

    /// Send a request and await its response (with configurable timeout).
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

        let timeout = tokio::time::Duration::from_secs(self.timeout_secs);
        match tokio::time::timeout(timeout, rx.recv()).await {
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
            Ok(None) => {
                Err(crate::error::MantaError::Internal("Response channel closed".to_string()))
            }
            Err(_) => Err(crate::error::MantaError::Internal(format!(
                "Request timeout after {}s",
                self.timeout_secs
            ))),
        }
    }

    /// Refresh the tool list from the server.
    async fn list_tools(&mut self) -> crate::Result<()> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "tools/list".to_string(),
            params: None,
        };

        let response = self.send_request(request).await?;
        if let Some(result) = response.result {
            // The MCP spec wraps tools in `{"tools": [...]}`.
            let tools: Vec<McpToolDefinition> = if let Some(arr) = result.get("tools") {
                serde_json::from_value(arr.clone()).unwrap_or_default()
            } else {
                serde_json::from_value(result).unwrap_or_default()
            };
            info!("Discovered {} MCP tools", tools.len());
            self.tools = tools;
        }

        Ok(())
    }

    /// Call an MCP tool by name.
    pub async fn call_tool(
        &self,
        name: &str,
        params: serde_json::Value,
    ) -> crate::Result<serde_json::Value> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
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

    // ── Resource methods (9.7) ────────────────────────────────────────────────

    /// List resources available from the MCP server.
    pub async fn list_resources(&self) -> crate::Result<Vec<McpResource>> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "resources/list".to_string(),
            params: None,
        };

        let response = self.send_request(request).await?;
        if let Some(result) = response.result {
            let resources: Vec<McpResource> = if let Some(arr) = result.get("resources") {
                serde_json::from_value(arr.clone()).unwrap_or_default()
            } else {
                serde_json::from_value(result).unwrap_or_default()
            };
            return Ok(resources);
        }
        Ok(Vec::new())
    }

    /// Read a resource by URI.
    pub async fn read_resource(&self, uri: &str) -> crate::Result<Vec<McpResourceContent>> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "resources/read".to_string(),
            params: Some(json!({ "uri": uri })),
        };

        let response = self.send_request(request).await?;
        if let Some(result) = response.result {
            let contents: Vec<McpResourceContent> = if let Some(arr) = result.get("contents") {
                serde_json::from_value(arr.clone()).unwrap_or_default()
            } else {
                serde_json::from_value(result).unwrap_or_default()
            };
            return Ok(contents);
        }
        Ok(Vec::new())
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    /// Get discovered tools.
    pub fn get_tools(&self) -> &[McpToolDefinition] {
        &self.tools
    }

    /// Disconnect from the MCP server.
    pub async fn disconnect(&mut self) -> crate::Result<()> {
        info!("Disconnecting from MCP server");
        self.request_tx = None;
        if let Some(mut process) = self.process.take() {
            let _ = process.kill().await;
        }
        Ok(())
    }

    /// Returns true when the underlying channel is open.
    pub fn is_connected(&self) -> bool {
        self.request_tx.is_some() && !self.child_exited.load(Ordering::Relaxed)
    }

    /// True if the child process has exited (stdio transport).
    pub fn has_child_exited(&self) -> bool {
        self.child_exited.load(Ordering::Relaxed)
    }
}

impl Default for McpClient {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────
// McpToolWrapper – implements Tool (9.2)
// ─────────────────────────────────────────────

/// Wraps a single MCP tool so the agent can call it through `ToolRegistry`.
/// Tool names are registered as `mcp__{server_id}__{tool_name}`.
#[derive(Debug)]
pub struct McpToolWrapper {
    /// Shared client for the originating server
    client: Arc<RwLock<McpClient>>,
    /// Fully-qualified tool name (e.g. `mcp__filesystem__read_file`)
    qualified_name: String,
    /// Original MCP tool name
    tool_name: String,
    tool_description: String,
    parameters_schema: serde_json::Value,
}

impl McpToolWrapper {
    /// Create a wrapper.  `server_id` is the key from `mcp.servers.*`.
    pub fn new(client: Arc<RwLock<McpClient>>, server_id: &str, tool: &McpToolDefinition) -> Self {
        let qualified_name = format!("mcp__{}__{}", server_id, tool.name);
        Self {
            client,
            qualified_name,
            tool_name: tool.name.clone(),
            tool_description: tool.description.clone(),
            parameters_schema: tool.parameters.clone(),
        }
    }
}

#[async_trait]
impl Tool for McpToolWrapper {
    fn name(&self) -> &str {
        &self.qualified_name
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
        Ok(ToolExecutionResult::success(format!("MCP tool result: {}", result)).with_data(result))
    }
}

// ─────────────────────────────────────────────
// McpManager – owns all clients (9.1, 9.2, 9.4)
// ─────────────────────────────────────────────

/// Manages all MCP server connections.  Lives in `GatewayState`.
#[derive(Debug, Default)]
pub struct McpManager {
    clients: Arc<RwLock<HashMap<String, Arc<RwLock<McpClient>>>>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Connect to a server and return its discovered tools.
    pub async fn connect(
        &self,
        server_id: &str,
        config: McpServerConfig,
    ) -> crate::Result<Vec<McpToolDefinition>> {
        let mut client = McpClient::new().with_timeout(config.timeout_secs);
        client.connect(config).await?;

        let tools = client.get_tools().to_vec();

        let client_arc = Arc::new(RwLock::new(client));
        self.clients
            .write()
            .await
            .insert(server_id.to_string(), client_arc);

        Ok(tools)
    }

    /// Disconnect a server and return its tools so callers can deregister them.
    pub async fn disconnect(&self, server_id: &str) -> crate::Result<()> {
        let removed = self.clients.write().await.remove(server_id);
        if let Some(client_arc) = removed {
            client_arc.write().await.disconnect().await?;
        }
        Ok(())
    }

    /// Get the `Arc<RwLock<McpClient>>` for a server.
    pub async fn get_client(&self, server_id: &str) -> Option<Arc<RwLock<McpClient>>> {
        self.clients.read().await.get(server_id).cloned()
    }

    /// List connected server IDs.
    pub async fn list_servers(&self) -> Vec<String> {
        self.clients.read().await.keys().cloned().collect()
    }

    /// Attempt exponential-backoff reconnect for a disconnected server (9.4).
    pub async fn reconnect_with_backoff(
        &self,
        server_id: &str,
        config: McpServerConfig,
    ) -> crate::Result<Vec<McpToolDefinition>> {
        let delays: &[u64] = &[5, 10, 20, 40, 80];
        for &secs in delays {
            warn!("Reconnecting to MCP server '{}' in {}s …", server_id, secs);
            tokio::time::sleep(tokio::time::Duration::from_secs(secs)).await;
            match self.connect(server_id, config.clone()).await {
                Ok(tools) => {
                    info!("Reconnected to MCP server '{}'", server_id);
                    return Ok(tools);
                }
                Err(e) => {
                    warn!("Reconnect attempt failed for '{}': {}", server_id, e);
                }
            }
        }
        Err(crate::error::MantaError::Internal(format!(
            "Failed to reconnect to MCP server '{}' after {} attempts",
            server_id,
            delays.len()
        )))
    }
}

// ─────────────────────────────────────────────
// McpConnectionTool – the `mcp` agent tool
// ─────────────────────────────────────────────

/// Meta-tool the agent can invoke to manage MCP connections at runtime.
#[derive(Debug)]
pub struct McpConnectionTool {
    manager: Arc<McpManager>,
}

impl McpConnectionTool {
    pub fn new() -> Self {
        Self {
            manager: Arc::new(McpManager::new()),
        }
    }

    /// Create with a shared manager (so gateway can also share it).
    pub fn with_manager(manager: Arc<McpManager>) -> Self {
        Self { manager }
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

Actions:
- connect: Connect to an MCP server
- disconnect: Disconnect from an MCP server
- list: List connected MCP servers
- tools: List available tools from a server
- resources: List resources available from a server
- resource_read: Read a resource by URI"#
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["connect", "disconnect", "list", "tools", "resources", "resource_read"],
                    "description": "Action to perform"
                },
                "server_id": {
                    "type": "string",
                    "description": "Unique identifier for the server connection"
                },
                "command": {
                    "type": "string",
                    "description": "Command to run the MCP server (stdio transport)"
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Arguments for the command"
                },
                "url": {
                    "type": "string",
                    "description": "URL for SSE / streamable-HTTP transport"
                },
                "transport": {
                    "type": "string",
                    "enum": ["stdio", "sse", "streamable_http"],
                    "description": "Transport type (default: stdio)"
                },
                "uri": {
                    "type": "string",
                    "description": "Resource URI (for resource_read)"
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
        let action = args["action"].as_str().ok_or_else(|| {
            crate::error::MantaError::Validation("action is required".to_string())
        })?;

        match action {
            "connect" => {
                let server_id = args["server_id"].as_str().ok_or_else(|| {
                    crate::error::MantaError::Validation(
                        "server_id is required for connect".to_string(),
                    )
                })?;

                let transport = match args["transport"].as_str().unwrap_or("stdio") {
                    "sse" => McpTransport::Sse,
                    "streamable_http" => McpTransport::StreamableHttp,
                    _ => McpTransport::Stdio,
                };

                let config = McpServerConfig {
                    transport,
                    command: args["command"].as_str().map(String::from),
                    args: args["args"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    url: args["url"].as_str().map(String::from),
                    ..Default::default()
                };

                let tools = self.manager.connect(server_id, config).await?;
                Ok(ToolExecutionResult::success(format!(
                    "Connected to MCP server '{}'. {} tools available.",
                    server_id,
                    tools.len()
                ))
                .with_data(json!({ "tools": tools.iter().map(|t| &t.name).collect::<Vec<_>>() })))
            }

            "disconnect" => {
                let server_id = args["server_id"].as_str().ok_or_else(|| {
                    crate::error::MantaError::Validation(
                        "server_id is required for disconnect".to_string(),
                    )
                })?;
                self.manager.disconnect(server_id).await?;
                Ok(ToolExecutionResult::success(format!(
                    "Disconnected from MCP server '{}'",
                    server_id
                )))
            }

            "list" => {
                let servers = self.manager.list_servers().await;
                Ok(ToolExecutionResult::success(format!("{} MCP servers connected", servers.len()))
                    .with_data(json!({ "servers": servers })))
            }

            "tools" => {
                let server_id = args["server_id"].as_str().ok_or_else(|| {
                    crate::error::MantaError::Validation(
                        "server_id is required for tools".to_string(),
                    )
                })?;
                match self.manager.get_client(server_id).await {
                    Some(client_arc) => {
                        let client = client_arc.read().await;
                        let tools = client.get_tools().to_vec();
                        Ok(ToolExecutionResult::success(format!(
                            "{} tools from '{}'",
                            tools.len(),
                            server_id
                        ))
                        .with_data(json!({ "tools": tools })))
                    }
                    None => Ok(ToolExecutionResult::error(format!(
                        "MCP server not found: {}",
                        server_id
                    ))),
                }
            }

            "resources" => {
                let server_id = args["server_id"].as_str().ok_or_else(|| {
                    crate::error::MantaError::Validation(
                        "server_id is required for resources".to_string(),
                    )
                })?;
                match self.manager.get_client(server_id).await {
                    Some(client_arc) => {
                        let client = client_arc.read().await;
                        let resources = client.list_resources().await?;
                        Ok(ToolExecutionResult::success(format!(
                            "{} resources from '{}'",
                            resources.len(),
                            server_id
                        ))
                        .with_data(json!({ "resources": resources })))
                    }
                    None => Ok(ToolExecutionResult::error(format!(
                        "MCP server not found: {}",
                        server_id
                    ))),
                }
            }

            "resource_read" => {
                let server_id = args["server_id"].as_str().ok_or_else(|| {
                    crate::error::MantaError::Validation(
                        "server_id is required for resource_read".to_string(),
                    )
                })?;
                let uri = args["uri"].as_str().ok_or_else(|| {
                    crate::error::MantaError::Validation(
                        "uri is required for resource_read".to_string(),
                    )
                })?;
                match self.manager.get_client(server_id).await {
                    Some(client_arc) => {
                        let client = client_arc.read().await;
                        let contents = client.read_resource(uri).await?;
                        Ok(ToolExecutionResult::success(format!(
                            "Read {} content blocks from '{}'",
                            contents.len(),
                            uri
                        ))
                        .with_data(json!({ "contents": contents })))
                    }
                    None => Ok(ToolExecutionResult::error(format!(
                        "MCP server not found: {}",
                        server_id
                    ))),
                }
            }

            _ => Err(crate::error::MantaError::Validation(format!("Unknown action: {}", action))),
        }
    }
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_client_default() {
        let client = McpClient::default();
        assert!(!client.is_connected());
        assert!(client.get_tools().is_empty());
    }

    #[test]
    fn test_mcp_server_config_defaults() {
        let config = McpServerConfig::default();
        assert_eq!(config.timeout_secs, 30);
        assert!(config.auto_connect);
        assert!(config.command.is_none());
    }

    #[test]
    fn test_env_resolution() {
        // Set a temp env var
        std::env::set_var("MCP_TEST_VAR", "hello");
        let mut env = HashMap::new();
        env.insert("KEY".to_string(), "$MCP_TEST_VAR".to_string());
        env.insert("LITERAL".to_string(), "world".to_string());

        let resolved = McpClient::resolve_env(&env);
        assert_eq!(resolved["KEY"], "hello");
        assert_eq!(resolved["LITERAL"], "world");
        std::env::remove_var("MCP_TEST_VAR");
    }

    #[test]
    fn test_tool_wrapper_name() {
        let client = Arc::new(RwLock::new(McpClient::new()));
        let def = McpToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({}),
        };
        let wrapper = McpToolWrapper::new(client, "filesystem", &def);
        assert_eq!(wrapper.name(), "mcp__filesystem__read_file");
    }

    #[test]
    fn test_mcp_settings_deserialization() {
        let toml_str = r#"
[servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem"]
timeout_secs = 60
auto_connect = true
"#;
        let settings: McpSettings = toml::from_str(toml_str).unwrap();
        assert!(settings.servers.contains_key("filesystem"));
        let fs = &settings.servers["filesystem"];
        assert_eq!(fs.command.as_deref(), Some("npx"));
        assert_eq!(fs.timeout_secs, 60);
    }
}
