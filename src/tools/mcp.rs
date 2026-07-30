//! MCP (Model Context Protocol) Integration
//!
//! This module implements a client for the Model Context Protocol,
//! allowing Syscity to connect to MCP servers and use their tools.
//!
//! Supported transports:
//! - `stdio` – spawn a subprocess and communicate over stdin/stdout
//! - `sse` – connect to an HTTP server via Server-Sent Events
//! - `streamable_http` – POST requests with SSE response bodies

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{error, info, warn};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;

use super::{Tool, ToolContext, ToolExecutionChunk, ToolExecutionResult};
use crate::tools::sdk::ToolCapabilities;

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

/// Per-server MCP configuration (used in config.toml `[mcp.servers.*]`)
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
    /// Environment variables (supports `$VAR` references — resolved at connect
    /// time)
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
    /// Health-check interval in seconds (0 disables health checks).
    #[serde(default = "default_health_check_interval_secs")]
    pub health_check_interval_secs: u64,
    /// Automatically reconnect when a connection is marked unhealthy.
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,
    /// Maximum reconnect attempts after a connection failure.
    #[serde(default = "default_max_reconnect_attempts")]
    pub max_reconnect_attempts: u32,
    /// OAuth 2.0 / bearer auth type (e.g. "oauth2", "bearer", or null)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_type: Option<String>,
    /// OAuth client ID (for oauth2 flow)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// OAuth authorization endpoint (auto-discovered if omitted)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    /// OAuth token endpoint (auto-discovered if omitted)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    /// OAuth scopes (space-separated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<String>,
}

fn default_health_check_interval_secs() -> u64 {
    30
}

fn default_max_reconnect_attempts() -> u32 {
    5
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
            health_check_interval_secs: default_health_check_interval_secs(),
            auto_reconnect: true,
            max_reconnect_attempts: default_max_reconnect_attempts(),
            auth_type: None,
            client_id: None,
            auth_url: None,
            token_url: None,
            scopes: None,
        }
    }
}

/// Top-level `[mcp]` section in config.toml / GatewayConfig
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
    method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
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

/// Tool capability details returned in the MCP `initialize` response.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpToolsCapability {
    /// The server supports `notifications/tools/list_changed`.
    #[serde(default, rename = "listChanged")]
    pub list_changed: bool,
}

/// Resource capability details returned in the MCP `initialize` response.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpResourcesCapability {
    /// The server supports `resources/subscribe` and resource update
    /// notifications.
    #[serde(default)]
    pub subscribe: bool,
    /// The server supports `notifications/resources/list_changed`.
    #[serde(default, rename = "listChanged")]
    pub list_changed: bool,
}

/// Prompt capability details returned in the MCP `initialize` response.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpPromptsCapability {
    /// The server supports `notifications/prompts/list_changed`.
    #[serde(default, rename = "listChanged")]
    pub list_changed: bool,
}

/// Server capabilities returned in the MCP `initialize` response.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpServerCapabilities {
    /// Tool support and sub-capabilities.
    #[serde(default)]
    pub tools: Option<McpToolsCapability>,
    /// Resource support and sub-capabilities.
    #[serde(default)]
    pub resources: Option<McpResourcesCapability>,
    /// Prompt support and sub-capabilities.
    #[serde(default)]
    pub prompts: Option<McpPromptsCapability>,
    /// Logging support (e.g. `setLevel`).
    #[serde(default)]
    pub logging: Option<serde_json::Value>,
    /// Any additional capabilities.
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl McpServerCapabilities {
    /// Returns true if the server supports tools.
    pub fn supports_tools(&self) -> bool {
        self.tools.is_some()
    }

    /// Returns true if the server supports tool list-change notifications.
    pub fn supports_tool_list_changed(&self) -> bool {
        self.tools.as_ref().map(|c| c.list_changed).unwrap_or(false)
    }

    /// Returns true if the server supports resources.
    pub fn supports_resources(&self) -> bool {
        self.resources.is_some()
    }

    /// Returns true if the server supports resource subscriptions.
    pub fn supports_resource_subscribe(&self) -> bool {
        self.resources
            .as_ref()
            .map(|c| c.subscribe)
            .unwrap_or(false)
    }

    /// Returns true if the server supports resource list-change notifications.
    pub fn supports_resource_list_changed(&self) -> bool {
        self.resources
            .as_ref()
            .map(|c| c.list_changed)
            .unwrap_or(false)
    }

    /// Returns true if the server supports prompts.
    pub fn supports_prompts(&self) -> bool {
        self.prompts.is_some()
    }

    /// Returns true if the server supports prompt list-change notifications.
    pub fn supports_prompt_list_changed(&self) -> bool {
        self.prompts
            .as_ref()
            .map(|c| c.list_changed)
            .unwrap_or(false)
    }
}

/// Full result of an MCP `initialize` handshake.
#[derive(Debug, Clone, Deserialize)]
struct McpInitializeResult {
    #[serde(rename = "serverInfo")]
    server_info: McpServerInfo,
    #[serde(default)]
    capabilities: McpServerCapabilities,
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
// Prompt types (2024-11-05 spec)
// ─────────────────────────────────────────────

/// Argument schema for an MCP prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

/// MCP prompt descriptor returned by `prompts/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<McpPromptArgument>>,
}

/// A single message inside a rendered prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptMessage {
    pub role: String,
    pub content: serde_json::Value,
}

/// Result of `prompts/get`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpGetPromptResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub messages: Vec<McpPromptMessage>,
}

// ─────────────────────────────────────────────
// Sampling types (2024-11-05 spec)
// ─────────────────────────────────────────────

/// A sampling message sent to the server via `sampling/createMessage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSamplingMessage {
    pub role: String,
    pub content: serde_json::Value,
}

/// Result of `sampling/createMessage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSamplingResult {
    pub role: String,
    pub content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

// ─────────────────────────────────────────────
// Server-initiated notifications (2024-11-05 spec)
// ─────────────────────────────────────────────

/// A server-initiated MCP notification.
#[derive(Debug, Clone)]
pub enum McpNotification {
    /// `notifications/resources/updated`
    ResourceUpdated { uri: String },
    /// `notifications/resources/list_changed`
    ResourceListChanged,
    /// `notifications/tools/list_changed`
    ToolListChanged,
    /// `notifications/progress`
    Progress {
        progress_token: serde_json::Value,
        progress: f64,
        total: Option<f64>,
    },
    /// `notifications/message`
    Message {
        level: String,
        data: serde_json::Value,
    },
    /// Any other notification, preserved as raw JSON.
    Other {
        method: String,
        params: Option<serde_json::Value>,
    },
}

// ─────────────────────────────────────────────
// OAuth 2.0 token types
// ─────────────────────────────────────────────

/// OAuth 2.0 token data persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

/// Pending OAuth authorization state.
#[derive(Debug)]
pub struct PendingAuth {
    pub server_id: String,
    pub token_url: String,
    pub code_verifier: String,
    pub state: String,
    pub callback_port: u16,
    pub cancel_tx: tokio::sync::oneshot::Sender<()>,
}

// ─────────────────────────────────────────────
// McpClient (9.1, 9.3, 9.4, 9.6, 9.8)
// ─────────────────────────────────────────────

/// MCP client – one instance per connected server
#[derive(Debug)]
pub struct McpClient {
    /// Kill signal for the server process watcher (stdio transport only).
    /// The watcher task owns the `Child` so it can await real process exit.
    kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Request sender channel (present when connected)
    request_tx: Option<mpsc::UnboundedSender<McpRequest>>,
    /// Notification sender channel for server-pushed messages.
    notification_tx: Option<mpsc::UnboundedSender<McpNotification>>,
    /// Broadcast channel for progress notifications used during streaming tool
    /// calls.
    progress_tx: Option<broadcast::Sender<McpNotification>>,
    /// Server metadata returned during `initialize`
    server_info: Option<McpServerInfo>,
    /// Server capabilities returned during `initialize`
    server_capabilities: Option<McpServerCapabilities>,
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
    /// Bearer access token for remote OAuth MCP servers.
    access_token: Option<String>,
}

impl McpClient {
    /// Create a new unconnected client
    pub fn new() -> Self {
        Self {
            kill_tx: None,
            request_tx: None,
            notification_tx: None,
            progress_tx: None,
            server_info: None,
            server_capabilities: None,
            tools: Vec::new(),
            request_id: AtomicU64::new(1),
            response_channels: Arc::new(RwLock::new(HashMap::new())),
            child_exited: Arc::new(AtomicBool::new(false)),
            timeout_secs: 30,
            server_config: None,
            access_token: None,
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

    /// Set the notification sender channel for server-pushed messages.
    pub fn set_notification_tx(&mut self, tx: mpsc::UnboundedSender<McpNotification>) {
        self.notification_tx = Some(tx);
    }

    /// Set the bearer access token for OAuth-authenticated HTTP transport.
    pub fn set_access_token(&mut self, token: String) {
        self.access_token = Some(token);
    }

    /// Set the broadcast channel used to distribute progress notifications.
    pub fn set_progress_tx(&mut self, tx: broadcast::Sender<McpNotification>) {
        self.progress_tx = Some(tx);
    }

    /// Parse a server-initiated JSON-RPC message into an `McpNotification`.
    fn parse_notification(response: &McpResponse) -> Option<McpNotification> {
        let method = response.method.as_deref()?;
        match method {
            "notifications/resources/updated" => {
                let uri = response
                    .params
                    .as_ref()
                    .and_then(|p| p.get("uri"))
                    .and_then(|v| v.as_str())
                    .map(String::from)?;
                Some(McpNotification::ResourceUpdated { uri })
            }
            "notifications/resources/list_changed" => Some(McpNotification::ResourceListChanged),
            "notifications/tools/list_changed" => Some(McpNotification::ToolListChanged),
            "notifications/progress" => {
                let params = response.params.as_ref()?;
                let progress_token = params.get("progressToken").cloned()?;
                let progress = params.get("progress").and_then(|v| v.as_f64())?;
                let total = params.get("total").and_then(|v| v.as_f64());
                Some(McpNotification::Progress {
                    progress_token,
                    progress,
                    total,
                })
            }
            "notifications/message" => {
                let params = response.params.as_ref()?;
                let level = params
                    .get("level")
                    .and_then(|v| v.as_str())
                    .unwrap_or("info")
                    .to_string();
                Some(McpNotification::Message { level, data: params.clone() })
            }
            method => Some(McpNotification::Other {
                method: method.to_string(),
                params: response.params.clone(),
            }),
        }
    }

    /// Forward a notification to the manager channel and the progress broadcast
    /// channel.
    fn emit_notification(
        notification: McpNotification,
        notification_tx: &Option<mpsc::UnboundedSender<McpNotification>>,
        progress_tx: &Option<broadcast::Sender<McpNotification>>,
    ) {
        if matches!(&notification, McpNotification::Progress { .. }) {
            if let Some(tx) = progress_tx {
                let _ = tx.send(notification.clone());
            }
        }
        if let Some(tx) = notification_tx {
            let _ = tx.send(notification);
        }
    }

    // ── Stdio transport ──────────────────────────────────────────────────────

    /// Connect via stdio subprocess (9.1, 9.3, 9.4, 9.8)
    pub async fn connect_stdio(&mut self, config: McpServerConfig) -> crate::Result<()> {
        let command = config.command.as_deref().ok_or_else(|| {
            crate::error::SyscityError::Internal(
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
            crate::error::SyscityError::Internal(format!("Failed to spawn MCP server: {}", e))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            crate::error::SyscityError::Internal("Failed to get stdin".to_string())
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            crate::error::SyscityError::Internal("Failed to get stdout".to_string())
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
        let notification_tx = self.notification_tx.clone();
        let progress_tx = self.progress_tx.clone();
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
                    } else {
                        if let Some(notification) = McpClient::parse_notification(&response) {
                            McpClient::emit_notification(
                                notification,
                                &notification_tx,
                                &progress_tx,
                            );
                        }
                    }
                }
            }
        });

        // Process-exit watcher (9.4): the watcher owns the child so it can
        // await real exit; `kill_tx` lets disconnect() terminate it.
        // Reset the exit flag first so reconnects start clean.
        self.child_exited.store(false, Ordering::Relaxed);
        let (kill_tx, mut kill_rx) = tokio::sync::oneshot::channel::<()>();
        self.kill_tx = Some(kill_tx);
        let child_exited_watcher = self.child_exited.clone();

        // Take stderr before moving child into the exit-watcher task.
        let stderr_handle = child.stderr.take();

        tokio::spawn(async move {
            let mut child = child;
            enum Outcome {
                Killed,
                Exited(std::io::Result<std::process::ExitStatus>),
            }
            let outcome = tokio::select! {
                _ = &mut kill_rx => Outcome::Killed,
                status = child.wait() => Outcome::Exited(status),
            };
            match outcome {
                Outcome::Killed => {
                    let _ = child.kill().await;
                }
                Outcome::Exited(Ok(s)) => {
                    warn!("MCP stdio server process exited: {}", s);
                }
                Outcome::Exited(Err(e)) => {
                    warn!("MCP stdio server wait failed: {}", e);
                }
            }
            child_exited_watcher.store(true, Ordering::Relaxed);
        });

        // Read stderr from the subprocess asynchronously and log it.
        // This gives users visibility into WHY the process failed
        // (e.g. missing env vars shown in the process's own error output).
        if let Some(stderr) = stderr_handle {
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut reader = tokio::io::BufReader::new(stderr);
                let mut line = String::new();
                let mut total_bytes = 0u64;
                while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                    total_bytes += line.len() as u64;
                    if total_bytes > 4096 {
                        warn!("MCP server stderr: ...(truncated)");
                        break;
                    }
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        warn!("MCP server stderr: {}", trimmed);
                    }
                    line.clear();
                }
            });
        }

        // Cache config for reconnect
        self.server_config = Some(config);

        // Initialize protocol
        self.initialize().await.map_err(|e| {
            if self.child_exited.load(Ordering::Relaxed) {
                crate::error::SyscityError::Internal(format!(
                    "MCP server process exited before initialization: {}",
                    e
                ))
            } else {
                e
            }
        })?;

        info!("Connected to MCP server via stdio");
        Ok(())
    }

    // ── SSE transport (9.6) ──────────────────────────────────────────────────

    /// Connect to an MCP server via Server-Sent Events
    pub async fn connect_sse(&mut self, config: McpServerConfig) -> crate::Result<()> {
        let url = config.url.as_deref().ok_or_else(|| {
            crate::error::SyscityError::Internal("SSE transport requires 'url' field".to_string())
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
                crate::error::SyscityError::Internal(format!("Failed to build HTTP client: {}", e))
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
        let notification_tx_sse = self.notification_tx.clone();
        let progress_tx_sse = self.progress_tx.clone();
        let access_token_sse = self.access_token.clone();
        tokio::spawn(async move {
            let mut builder = client.get(&get_url).header("Accept", "text/event-stream");
            for (k, v) in &env_headers {
                builder = builder.header(k, v);
            }
            if let Some(ref token) = access_token_sse {
                builder = builder.header("Authorization", format!("Bearer {}", token));
            }
            let resp = match builder.send().await {
                Ok(r) => r,
                Err(e) => {
                    error!("SSE connection error: {}", e);
                    return;
                }
            };

            use futures::StreamExt;
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
                                        } else {
                                            if let Some(notification) =
                                                McpClient::parse_notification(&response)
                                            {
                                                McpClient::emit_notification(
                                                    notification,
                                                    &notification_tx_sse,
                                                    &progress_tx_sse,
                                                );
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
                crate::error::SyscityError::Internal(format!("Failed to build HTTP client: {}", e))
            })?;
        let env_for_writer = resolved_env.clone();
        let access_token_writer = self.access_token.clone();
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
                if let Some(ref token) = access_token_writer {
                    builder = builder.header("Authorization", format!("Bearer {}", token));
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
            crate::error::SyscityError::Internal(
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
        let notification_tx_http = self.notification_tx.clone();
        let progress_tx_http = self.progress_tx.clone();
        let access_token = self.access_token.clone();

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
                if let Some(ref token) = access_token {
                    builder = builder.header("Authorization", format!("Bearer {}", token));
                }

                let resp = match builder.send().await {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Failed to POST MCP request: {}", e);
                        continue;
                    }
                };

                use futures::StreamExt;
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
                                            } else {
                                                if let Some(notification) =
                                                    McpClient::parse_notification(&response)
                                                {
                                                    McpClient::emit_notification(
                                                        notification,
                                                        &notification_tx_http,
                                                        &progress_tx_http,
                                                    );
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
                    "tools": { "listChanged": true },
                    "resources": { "subscribe": true, "listChanged": true },
                    "prompts": { "listChanged": true },
                    "sampling": {},
                    "roots": { "listChanged": true },
                },
                "clientInfo": {
                    "name": "syscity",
                    "version": crate::VERSION,
                },
            })),
        };

        let response = self.send_request(request).await?;

        if let Some(result) = response.result {
            if let Ok(init_result) = serde_json::from_value::<McpInitializeResult>(result) {
                info!(
                    "MCP server: {} v{}",
                    init_result.server_info.name, init_result.server_info.version
                );
                self.server_info = Some(init_result.server_info);
                self.server_capabilities = Some(init_result.capabilities);
            }
        }

        // Only list tools if the server advertises tool support.
        if self
            .server_capabilities
            .as_ref()
            .map(|c| c.supports_tools())
            .unwrap_or(true)
        {
            self.list_tools().await?;
        } else {
            info!("MCP server does not advertise tool support; skipping tools/list");
        }

        // Discover resources if supported.
        if self
            .server_capabilities
            .as_ref()
            .map(|c| c.supports_resources())
            .unwrap_or(false)
        {
            // Keep resource discovery lazy; list_resources() is available on
            // demand.
        }

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
                crate::error::SyscityError::Internal("Request channel closed".to_string())
            })?;
        } else {
            return Err(crate::error::SyscityError::Internal("Not connected".to_string()));
        }

        let timeout = tokio::time::Duration::from_secs(self.timeout_secs);
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(response)) => {
                let mut channels = self.response_channels.write().await;
                channels.remove(&id);

                if let Some(error) = response.error {
                    return Err(crate::error::SyscityError::ExternalService {
                        source: format!("MCP error {}: {}", error.code, error.message),
                        cause: None,
                    });
                }

                Ok(response)
            }
            Ok(None) => {
                Err(crate::error::SyscityError::Internal("Response channel closed".to_string()))
            }
            Err(_) => Err(crate::error::SyscityError::Internal(format!(
                "Request timeout after {}s",
                self.timeout_secs
            ))),
        }
    }

    /// Refresh the tool list from the server.
    pub async fn list_tools(&mut self) -> crate::Result<()> {
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
            crate::error::SyscityError::Internal("No result from tool call".to_string())
        })
    }

    /// List prompts available from the MCP server.
    pub async fn list_prompts(&self) -> crate::Result<Vec<McpPrompt>> {
        if !self
            .server_capabilities
            .as_ref()
            .map(|c| c.supports_prompts())
            .unwrap_or(false)
        {
            return Ok(Vec::new());
        }

        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "prompts/list".to_string(),
            params: None,
        };

        let response = self.send_request(request).await?;
        if let Some(result) = response.result {
            let prompts: Vec<McpPrompt> = if let Some(arr) = result.get("prompts") {
                serde_json::from_value(arr.clone()).unwrap_or_default()
            } else {
                serde_json::from_value(result).unwrap_or_default()
            };
            return Ok(prompts);
        }
        Ok(Vec::new())
    }

    /// Render a prompt by name with optional arguments.
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<HashMap<String, String>>,
    ) -> crate::Result<McpGetPromptResult> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let mut params = json!({ "name": name });
        if let Some(args) = arguments {
            if let Some(obj) = params.as_object_mut() {
                obj.insert("arguments".to_string(), json!(args));
            }
        }

        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "prompts/get".to_string(),
            params: Some(params),
        };

        let response = self.send_request(request).await?;
        let result = response.result.ok_or_else(|| {
            crate::error::SyscityError::Internal("No result from prompts/get".to_string())
        })?;
        serde_json::from_value::<McpGetPromptResult>(result).map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to parse prompt result: {}", e))
        })
    }

    /// Ask the MCP server to sample a message (server-initiated LLM call).
    pub async fn sampling_create_message(
        &self,
        messages: Vec<McpSamplingMessage>,
        max_tokens: i64,
        model_hints: Option<Vec<String>>,
    ) -> crate::Result<McpSamplingResult> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let mut params = json!({
            "messages": messages,
            "maxTokens": max_tokens,
        });
        if let Some(hints) = model_hints {
            if let Some(obj) = params.as_object_mut() {
                obj.insert("modelPreferences".to_string(), json!({ "hints": hints }));
            }
        }

        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "sampling/createMessage".to_string(),
            params: Some(params),
        };

        let response = self.send_request(request).await?;
        let result = response.result.ok_or_else(|| {
            crate::error::SyscityError::Internal(
                "No result from sampling/createMessage".to_string(),
            )
        })?;
        serde_json::from_value::<McpSamplingResult>(result).map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to parse sampling result: {}", e))
        })
    }

    /// Call an MCP tool by name with a progress token for streaming
    /// notifications.
    pub async fn call_tool_with_progress(
        &self,
        name: &str,
        params: serde_json::Value,
        progress_token: &str,
    ) -> crate::Result<serde_json::Value> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let mut request_params = json!({
            "name": name,
            "arguments": params,
        });
        if let Some(obj) = request_params.as_object_mut() {
            obj.insert("_meta".to_string(), json!({ "progressToken": progress_token }));
        }

        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "tools/call".to_string(),
            params: Some(request_params),
        };

        let response = self.send_request(request).await?;
        response.result.ok_or_else(|| {
            crate::error::SyscityError::Internal("No result from tool call".to_string())
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

    /// Subscribe to resource change notifications for a URI.
    pub async fn subscribe_resource(&self, uri: &str) -> crate::Result<()> {
        if !self
            .server_capabilities
            .as_ref()
            .map(|c| c.supports_resource_subscribe())
            .unwrap_or(false)
        {
            return Err(crate::error::SyscityError::Internal(
                "MCP server does not support resource subscriptions".to_string(),
            ));
        }

        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "resources/subscribe".to_string(),
            params: Some(json!({ "uri": uri })),
        };

        let response = self.send_request(request).await?;
        if response.error.is_some() {
            return Err(crate::error::SyscityError::ExternalService {
                source: "resources/subscribe failed".to_string(),
                cause: response.error.map(|e| {
                    Box::new(std::io::Error::other(e.message))
                        as Box<dyn std::error::Error + Send + Sync>
                }),
            });
        }
        Ok(())
    }

    /// Unsubscribe from resource change notifications for a URI.
    pub async fn unsubscribe_resource(&self, uri: &str) -> crate::Result<()> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "resources/unsubscribe".to_string(),
            params: Some(json!({ "uri": uri })),
        };

        let response = self.send_request(request).await?;
        if response.error.is_some() {
            return Err(crate::error::SyscityError::ExternalService {
                source: "resources/unsubscribe failed".to_string(),
                cause: response.error.map(|e| {
                    Box::new(std::io::Error::other(e.message))
                        as Box<dyn std::error::Error + Send + Sync>
                }),
            });
        }
        Ok(())
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    /// Get discovered tools.
    pub fn get_tools(&self) -> &[McpToolDefinition] {
        &self.tools
    }

    /// Get the server capabilities returned during initialization.
    pub fn server_capabilities(&self) -> Option<&McpServerCapabilities> {
        self.server_capabilities.as_ref()
    }

    /// Disconnect from the MCP server.
    pub async fn disconnect(&mut self) -> crate::Result<()> {
        info!("Disconnecting from MCP server");
        self.request_tx = None;
        if let Some(kill_tx) = self.kill_tx.take() {
            let _ = kill_tx.send(());
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

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: true,
            risk_level: crate::tools::approval::RiskLevel::High,
            categories: vec!["system".to_string(), "mcp".to_string()],
            ..Default::default()
        }
    }

    fn is_available(&self, context: &ToolContext) -> bool {
        !context.sandboxed() || !context.allowed_commands().is_empty()
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

    fn execute_stream<'a>(
        &'a self,
        args: serde_json::Value,
        _context: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn tokio_stream::Stream<Item = ToolExecutionChunk> + Send + 'a>> {
        let client = self.client.clone();
        let tool_name = self.tool_name.clone();
        Box::pin(async_stream::stream! {
            let progress_token = uuid::Uuid::new_v4().to_string();
            let (result_tx, mut result_rx) = mpsc::unbounded_channel::<crate::Result<serde_json::Value>>();

            // Subscribe to progress notifications before issuing the call.
            let mut progress_rx = {
                let c = client.read().await;
                match c.progress_tx.as_ref() {
                    Some(tx) => tx.subscribe(),
                    None => {
                        // Progress streaming not wired; fall back to buffered execution.
                        match c.call_tool(&tool_name, args).await {
                            Ok(result) => {
                                yield ToolExecutionChunk::Output(format!("MCP tool result: {}", result));
                                yield ToolExecutionChunk::Data(result);
                            }
                            Err(e) => yield ToolExecutionChunk::Error(e.to_string()),
                        }
                        return;
                    }
                }
            };

            // Spawn the actual tool call with a progress token.
            let token = progress_token.clone();
            let call_client = client.clone();
            tokio::spawn(async move {
                let c = call_client.read().await;
                let result = c.call_tool_with_progress(&tool_name, args, &token).await;
                let _ = result_tx.send(result);
            });

            loop {
                tokio::select! {
                    maybe_notification = progress_rx.recv() => {
                        match maybe_notification {
                            Ok(McpNotification::Progress { progress_token: token, progress, total }) => {
                                if token.as_str() == Some(progress_token.as_str()) {
                                    let msg = match total {
                                        Some(t) => format!("Progress: {}/{}", progress, t),
                                        None => format!("Progress: {}", progress),
                                    };
                                    yield ToolExecutionChunk::Output(msg);
                                }
                            }
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                    maybe_result = result_rx.recv() => {
                        match maybe_result {
                            Some(Ok(result)) => {
                                yield ToolExecutionChunk::Output(format!("MCP tool result: {}", result));
                                yield ToolExecutionChunk::Data(result);
                                yield ToolExecutionChunk::Done;
                                break;
                            }
                            Some(Err(e)) => {
                                yield ToolExecutionChunk::Error(e.to_string());
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
        })
    }
}

// ─────────────────────────────────────────────
// McpPromptTool – implements Tool for MCP prompts
// ─────────────────────────────────────────────

/// Wraps a single MCP prompt so the agent can render it through `ToolRegistry`.
/// Prompt names are registered as `mcp__{server_id}__prompt__{prompt_name}`.
#[derive(Debug)]
pub struct McpPromptTool {
    /// Shared client for the originating server
    client: Arc<RwLock<McpClient>>,
    /// Fully-qualified tool name
    qualified_name: String,
    /// Original MCP prompt name
    prompt_name: String,
    prompt_description: String,
    /// JSON schema for the prompt arguments.
    parameters_schema: serde_json::Value,
}

impl McpPromptTool {
    /// Create a wrapper.  `server_id` is the key from `mcp.servers.*`.
    pub fn new(client: Arc<RwLock<McpClient>>, server_id: &str, prompt: &McpPrompt) -> Self {
        let qualified_name = format!("mcp__{}__prompt__{}", server_id, prompt.name);

        // Build a JSON schema from the prompt arguments.
        let properties = prompt
            .arguments
            .as_ref()
            .map(|args| {
                let mut props = serde_json::Map::new();
                let mut required = Vec::new();
                for arg in args {
                    let mut prop = serde_json::Map::new();
                    prop.insert("type".to_string(), json!("string"));
                    if let Some(desc) = &arg.description {
                        prop.insert("description".to_string(), json!(desc));
                    }
                    props.insert(arg.name.clone(), serde_json::Value::Object(prop));
                    if arg.required {
                        required.push(arg.name.clone());
                    }
                }
                let mut schema = serde_json::Map::new();
                schema.insert("type".to_string(), json!("object"));
                schema.insert("properties".to_string(), serde_json::Value::Object(props));
                if !required.is_empty() {
                    schema.insert("required".to_string(), json!(required));
                }
                serde_json::Value::Object(schema)
            })
            .unwrap_or_else(|| json!({ "type": "object" }));

        Self {
            client,
            qualified_name,
            prompt_name: prompt.name.clone(),
            prompt_description: prompt.description.clone().unwrap_or_default(),
            parameters_schema: properties,
        }
    }
}

#[async_trait]
impl Tool for McpPromptTool {
    fn name(&self) -> &str {
        &self.qualified_name
    }

    fn description(&self) -> &str {
        &self.prompt_description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.parameters_schema.clone()
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: false,
            risk_level: crate::tools::approval::RiskLevel::Medium,
            categories: vec!["mcp".to_string(), "prompt".to_string()],
            ..Default::default()
        }
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let arguments = args.as_object().map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect::<HashMap<_, _>>()
        });
        let client = self.client.read().await;
        let result = client.get_prompt(&self.prompt_name, arguments).await?;
        Ok(ToolExecutionResult::success(format!("MCP prompt result: {:?}", result))
            .with_data(serde_json::to_value(result).unwrap_or_default()))
    }
}

// ─────────────────────────────────────────────
// McpManager – owns all clients (9.1, 9.2, 9.4)
// ─────────────────────────────────────────────

/// Lifecycle events emitted by `McpManager`.
#[derive(Debug, Clone)]
pub enum McpEvent {
    /// A server connected successfully.
    Connected {
        server_id: String,
        tools: usize,
        prompts: usize,
        resources: usize,
    },
    /// A server disconnected or was marked unhealthy.
    Disconnected { server_id: String, reason: String },
    /// A server recovered after an automatic reconnect.
    Recovered { server_id: String, attempt: u32 },
    /// A subscribed resource changed on the server.
    ResourceChanged { server_id: String, uri: String },
    /// OAuth authorization is required to connect.
    AuthRequired { server_id: String, auth_url: String },
    /// OAuth authorization completed successfully.
    AuthComplete { server_id: String },
    /// OAuth authorization failed.
    AuthFailed { server_id: String, reason: String },
}

/// Health status of a single MCP server connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpHealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Mutable health record for one MCP connection.
#[derive(Debug)]
pub struct McpHealth {
    pub status: McpHealthStatus,
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    pub consecutive_failures: u32,
}

impl McpHealth {
    pub fn new() -> Self {
        Self {
            status: McpHealthStatus::Healthy,
            last_heartbeat: chrono::Utc::now(),
            consecutive_failures: 0,
        }
    }
}

impl Default for McpHealth {
    fn default() -> Self {
        Self::new()
    }
}

/// Metadata kept for each connected MCP server.
#[derive(Debug)]
pub struct McpConnectionMeta {
    client: Arc<RwLock<McpClient>>,
    config: McpServerConfig,
    health: Arc<RwLock<McpHealth>>,
    crash_count: AtomicU32,
}

impl McpConnectionMeta {
    fn new(client: Arc<RwLock<McpClient>>, config: McpServerConfig) -> Self {
        Self {
            client,
            config,
            health: Arc::new(RwLock::new(McpHealth::new())),
            crash_count: AtomicU32::new(0),
        }
    }
}

/// Manages all MCP server connections.  Lives in `GatewayState`.
#[derive(Debug)]
pub struct McpManager {
    clients: Arc<RwLock<HashMap<String, McpConnectionMeta>>>,
    event_tx: Arc<RwLock<Option<mpsc::UnboundedSender<McpEvent>>>>,
    /// Pending OAuth authorization flows keyed by server_id.
    pending_auths: Arc<RwLock<HashMap<String, PendingAuth>>>,
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            event_tx: Arc::new(RwLock::new(None)),
            pending_auths: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set the event sender used to emit MCP lifecycle events.
    pub async fn with_event_tx(self, tx: mpsc::UnboundedSender<McpEvent>) -> Self {
        *self.event_tx.write().await = Some(tx);
        self
    }

    async fn emit_event(&self, event: McpEvent) {
        if let Some(tx) = self.event_tx.read().await.as_ref() {
            let _ = tx.send(event);
        }
    }

    /// Register a pre-authenticated, already-connected client.
    /// Used by the OAuth flow after token exchange completes.
    pub async fn register_client(
        &self,
        server_id: &str,
        client: Arc<RwLock<McpClient>>,
        config: McpServerConfig,
    ) -> crate::Result<()> {
        let tools = {
            let c = client.read().await;
            c.get_tools().to_vec()
        };
        let prompts = {
            let c = client.read().await;
            c.list_prompts().await.unwrap_or_default()
        };
        let resources = {
            let c = client.read().await;
            c.list_resources().await.unwrap_or_default()
        };

        let meta = McpConnectionMeta::new(client.clone(), config.clone());

        // Wire notification and progress channels.
        let (notification_tx, mut notification_rx) =
            mpsc::unbounded_channel::<McpNotification>();
        let (progress_tx, _progress_rx) = broadcast::channel::<McpNotification>(128);
        {
            let mut c = client.write().await;
            c.set_notification_tx(notification_tx);
            c.set_progress_tx(progress_tx);
        }

        let server_id_owned = server_id.to_string();
        let clients_for_notifications = self.clients.clone();
        let event_tx_for_notifications = self.event_tx.clone();
        tokio::spawn(async move {
            while let Some(notification) = notification_rx.recv().await {
                match notification {
                    McpNotification::ResourceUpdated { uri } => {
                        if let Some(tx) = event_tx_for_notifications.read().await.as_ref() {
                            let _ = tx.send(McpEvent::ResourceChanged {
                                server_id: server_id_owned.clone(),
                                uri,
                            });
                        }
                    }
                    McpNotification::ToolListChanged => {
                        if let Some(meta) =
                            clients_for_notifications.read().await.get(&server_id_owned)
                        {
                            let c = meta.client.read().await;
                            if c.server_capabilities()
                                .map(|c| c.supports_tools())
                                .unwrap_or(false)
                            {
                                let client_clone = meta.client.clone();
                                let sid = server_id_owned.clone();
                                tokio::spawn(async move {
                                    let mut c = client_clone.write().await;
                                    if let Err(e) = c.list_tools().await {
                                        warn!("Failed to refresh tools for '{}': {}", sid, e);
                                    }
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

        self.clients
            .write()
            .await
            .insert(server_id.to_string(), meta);

        self.emit_event(McpEvent::Connected {
            server_id: server_id.to_string(),
            tools: tools.len(),
            prompts: prompts.len(),
            resources: resources.len(),
        })
        .await;

        self.start_health_monitor(server_id);

        Ok(())
    }

    /// Connect to a server and return its discovered tools.
    pub async fn connect(
        &self,
        server_id: &str,
        config: McpServerConfig,
    ) -> crate::Result<Vec<McpToolDefinition>> {
        let mut client = McpClient::new().with_timeout(config.timeout_secs);
        client.connect(config.clone()).await?;

        let tools = client.get_tools().to_vec();
        let prompts = client.list_prompts().await.unwrap_or_default();
        let resources = client.list_resources().await.unwrap_or_default();

        let client_arc = Arc::new(RwLock::new(client));
        let meta = McpConnectionMeta::new(client_arc.clone(), config.clone());

        // Wire notification and progress channels.
        let (notification_tx, mut notification_rx) = mpsc::unbounded_channel::<McpNotification>();
        let (progress_tx, _progress_rx) = broadcast::channel::<McpNotification>(128);
        {
            let mut c = client_arc.write().await;
            c.set_notification_tx(notification_tx);
            c.set_progress_tx(progress_tx);
        }

        let server_id_owned = server_id.to_string();
        let clients_for_notifications = self.clients.clone();
        let event_tx_for_notifications = self.event_tx.clone();
        tokio::spawn(async move {
            while let Some(notification) = notification_rx.recv().await {
                match notification {
                    McpNotification::ResourceUpdated { uri } => {
                        if let Some(tx) = event_tx_for_notifications.read().await.as_ref() {
                            let _ = tx.send(McpEvent::ResourceChanged {
                                server_id: server_id_owned.clone(),
                                uri,
                            });
                        }
                    }
                    McpNotification::ToolListChanged => {
                        // Refresh tool list in the background.
                        if let Some(meta) =
                            clients_for_notifications.read().await.get(&server_id_owned)
                        {
                            let c = meta.client.read().await;
                            if c.server_capabilities()
                                .map(|c| c.supports_tools())
                                .unwrap_or(false)
                            {
                                // list_tools requires &mut self; spawn a short task with a clone of
                                // the Arc.
                                let client_clone = meta.client.clone();
                                let sid = server_id_owned.clone();
                                tokio::spawn(async move {
                                    let mut c = client_clone.write().await;
                                    if let Err(e) = c.list_tools().await {
                                        warn!("Failed to refresh tools for '{}': {}", sid, e);
                                    }
                                });
                            }
                        }
                    }
                    McpNotification::ResourceListChanged => {
                        // Nothing automatic to do; consumers can re-list on
                        // demand.
                    }
                    _ => {}
                }
            }
        });

        self.clients
            .write()
            .await
            .insert(server_id.to_string(), meta);

        self.emit_event(McpEvent::Connected {
            server_id: server_id.to_string(),
            tools: tools.len(),
            prompts: prompts.len(),
            resources: resources.len(),
        })
        .await;

        // Start health monitor for this connection.
        self.start_health_monitor(server_id);

        Ok(tools)
    }

    /// Disconnect a server.
    pub async fn disconnect(&self, server_id: &str) -> crate::Result<()> {
        let removed = self.clients.write().await.remove(server_id);
        if let Some(meta) = removed {
            meta.client.write().await.disconnect().await?;
            self.emit_event(McpEvent::Disconnected {
                server_id: server_id.to_string(),
                reason: "manual_disconnect".to_string(),
            })
            .await;
        }
        Ok(())
    }

    /// Get the `Arc<RwLock<McpClient>>` for a server.
    pub async fn get_client(&self, server_id: &str) -> Option<Arc<RwLock<McpClient>>> {
        self.clients
            .read()
            .await
            .get(server_id)
            .map(|m| m.client.clone())
    }

    /// Get the health record for a server.
    pub async fn get_health(&self, server_id: &str) -> Option<Arc<RwLock<McpHealth>>> {
        self.clients
            .read()
            .await
            .get(server_id)
            .map(|m| m.health.clone())
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
        let max_attempts = config.max_reconnect_attempts.max(1) as usize;
        let base_delays: &[u64] = &[5, 10, 20, 40, 80];
        let delays: Vec<u64> = base_delays
            .iter()
            .cycle()
            .take(max_attempts)
            .copied()
            .collect();

        for (attempt, &secs) in delays.iter().enumerate() {
            warn!(
                "Reconnecting to MCP server '{}' in {}s (attempt {}/{}) …",
                server_id,
                secs,
                attempt + 1,
                delays.len()
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(secs)).await;
            match self.connect(server_id, config.clone()).await {
                Ok(tools) => {
                    info!("Reconnected to MCP server '{}'", server_id);
                    if let Some(meta) = self.clients.read().await.get(server_id) {
                        meta.crash_count.store(0, Ordering::SeqCst);
                    }
                    self.emit_event(McpEvent::Recovered {
                        server_id: server_id.to_string(),
                        attempt: (attempt + 1) as u32,
                    })
                    .await;
                    return Ok(tools);
                }
                Err(e) => {
                    warn!("Reconnect attempt failed for '{}': {}", server_id, e);
                }
            }
        }
        Err(crate::error::SyscityError::Internal(format!(
            "Failed to reconnect to MCP server '{}' after {} attempts",
            server_id,
            delays.len()
        )))
    }

    /// Spawn a background health monitor for a single server connection.
    fn start_health_monitor(&self, server_id: &str) -> tokio::task::JoinHandle<()> {
        let server_id = server_id.to_string();
        let clients = self.clients.clone();
        let event_tx = self.event_tx.clone();

        tokio::spawn(async move {
            loop {
                let interval_secs = {
                    let guard = clients.read().await;
                    guard
                        .get(&server_id)
                        .map(|m| m.config.health_check_interval_secs.max(5))
                        .unwrap_or(0)
                };
                if interval_secs == 0 {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;

                let reconnect_config = {
                    let mut guard = clients.write().await;
                    let Some(meta) = guard.get_mut(&server_id) else {
                        break;
                    };

                    let client = meta.client.read().await;
                    let healthy = client.is_connected() && !client.has_child_exited();
                    drop(client);

                    let mut health = meta.health.write().await;
                    if healthy {
                        health.status = McpHealthStatus::Healthy;
                        health.last_heartbeat = chrono::Utc::now();
                        health.consecutive_failures = 0;
                        None
                    } else {
                        health.consecutive_failures += 1;
                        health.status = if health.consecutive_failures == 1 {
                            McpHealthStatus::Degraded
                        } else {
                            McpHealthStatus::Unhealthy
                        };
                        let failures = health.consecutive_failures;
                        drop(health);
                        let auto_reconnect = meta.config.auto_reconnect;
                        let config = meta.config.clone();
                        drop(guard);

                        warn!(
                            "MCP server '{}' health check failed ({} consecutive)",
                            server_id, failures
                        );

                        if failures >= 2 && auto_reconnect {
                            Some(config)
                        } else {
                            None
                        }
                    }
                };

                if let Some(config) = reconnect_config {
                    if let Some(tx) = event_tx.read().await.as_ref() {
                        let _ = tx.send(McpEvent::Disconnected {
                            server_id: server_id.clone(),
                            reason: "health_check_failed".to_string(),
                        });
                    }

                    warn!("MCP server '{}' marked unhealthy; attempting recovery", server_id);

                    let _ = clients.write().await.remove(&server_id);

                    let manager = McpManager {
                        clients: clients.clone(),
                        event_tx: event_tx.clone(),
                        pending_auths: Arc::new(RwLock::new(HashMap::new())),
                    };
                    if let Err(e) = manager.reconnect_with_backoff(&server_id, config).await {
                        error!("MCP server '{}' recovery failed: {}", server_id, e);
                    }
                    break;
                }
            }
        })
    }
}

// ─────────────────────────────────────────────
// OAuth token persistence
// ─────────────────────────────────────────────

/// Directory for MCP OAuth token storage (~/.syscity/mcp_tokens).
pub fn mcp_tokens_dir() -> PathBuf {
    crate::dirs::syscity_dir().join("mcp_tokens")
}

/// Path to the token file for a specific server.
pub fn token_path_for(server_id: &str) -> PathBuf {
    mcp_tokens_dir().join(format!("{}.json", server_id))
}

/// Minimal percent-encoding for OAuth URL parameters.
fn urlencoding(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => result.push_str(&format!("%{byte:02X}")),
        }
    }
    result
}

impl McpManager {
    /// Load stored OAuth tokens for a server.
    pub async fn load_stored_token(&self, server_id: &str) -> Option<OAuthTokens> {
        let path = token_path_for(server_id);
        let data = tokio::fs::read_to_string(&path).await.ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Check if stored OAuth tokens exist and are not expired.
    pub async fn has_stored_token(&self, server_id: &str) -> bool {
        match self.load_stored_token(server_id).await {
            Some(tokens) => {
                if let Some(expires_at) = tokens.expires_at {
                    let now = chrono::Utc::now().timestamp();
                    if now >= expires_at - 60 {
                        return false;
                    }
                }
                true
            }
            None => false,
        }
    }

    /// Start the OAuth 2.1 + PKCE authorization flow for a remote MCP server.
    ///
    /// Returns the authorization URL the frontend should open in a browser.
    pub async fn start_oauth_flow(
        &self,
        server_id: &str,
        config: &McpServerConfig,
    ) -> crate::Result<String> {
        let url = config.url.as_deref().ok_or_else(|| {
            crate::error::SyscityError::Internal("Remote MCP server has no URL".to_string())
        })?;

        // 1. Discover OAuth endpoints via well-known URL
        let origin = Self::origin_from_url(url);
        let well_known_url = format!("{origin}/.well-known/oauth-authorization-server");

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!(
                    "Failed to build HTTP client: {e}"
                ))
            })?;

        let discovery_response = http_client
            .get(&well_known_url)
            .send()
            .await
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!(
                    "OAuth discovery failed for '{server_id}': {e}"
                ))
            })?;

        if !discovery_response.status().is_success() {
            return Err(crate::error::SyscityError::Internal(format!(
                "OAuth discovery failed for '{server_id}': HTTP {}",
                discovery_response.status()
            )));
        }

        let discovery: serde_json::Value = discovery_response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!(
                "Failed to parse OAuth discovery document: {e}"
            ))
        })?;

        let authorization_endpoint = discovery["authorization_endpoint"]
            .as_str()
            .ok_or_else(|| {
                crate::error::SyscityError::Internal(
                    "Missing authorization_endpoint in OAuth discovery".to_string(),
                )
            })?
            .to_string();

        let token_endpoint = discovery["token_endpoint"].as_str().ok_or_else(|| {
            crate::error::SyscityError::Internal(
                "Missing token_endpoint in OAuth discovery".to_string(),
            )
        })?.to_string();

        // 2. Generate PKCE challenge
        let code_verifier = Self::generate_code_verifier();
        let code_challenge = Self::generate_code_challenge(&code_verifier);
        let state = Self::generate_random_state();

        let client_id = config
            .client_id
            .clone()
            .unwrap_or_else(|| "syscity".to_string());

        let scopes = config.scopes.clone().unwrap_or_default();

        // 3. Bind local callback listener
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to bind callback port: {e}"))
        })?;
        let callback_port = listener.local_addr().map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to get local addr: {e}"))
        })?.port();

        let redirect_uri_filled = format!("http://127.0.0.1:{callback_port}/callback");

        // 4. Build authorization URL
        let mut auth_url = format!(
            "{authorization_endpoint}?response_type=code&client_id={}&redirect_uri={}&code_challenge={code_challenge}&code_challenge_method=S256&state={state}",
            urlencoding(&client_id),
            urlencoding(&redirect_uri_filled),
        );
        if !scopes.is_empty() {
            auth_url.push_str(&format!("&scope={}", urlencoding(&scopes)));
        }

        // 5. Create cancel/completion channels
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

        // 6. Store pending auth
        {
            let mut pending = self.pending_auths.write().await;
            pending.insert(
                server_id.to_string(),
                PendingAuth {
                    server_id: server_id.to_string(),
                    token_url: token_endpoint.clone(),
                    code_verifier: code_verifier.clone(),
                    state: state.clone(),
                    callback_port,
                    cancel_tx,
                },
            );
        }

        // 7. Spawn callback server task
        let server_id_clone = server_id.to_string();
        let token_url_clone = token_endpoint;
        let code_verifier_clone = code_verifier;
        let state_clone = state;
        let event_tx = self.event_tx.clone();
        let pending_auths = self.pending_auths.clone();
        let tokens_dir = mcp_tokens_dir();

        tokio::spawn(async move {
            let result = Self::run_callback_server(
                listener,
                &token_url_clone,
                &code_verifier_clone,
                &state_clone,
                &client_id,
                &redirect_uri_filled,
                cancel_rx,
            )
            .await;

            match result {
                Ok(tokens) => {
                    let _ = tokio::fs::create_dir_all(&tokens_dir).await;
                    let token_path = tokens_dir.join(format!("{server_id_clone}.json"));
                    if let Ok(json) = serde_json::to_string(&tokens) {
                        let _ = tokio::fs::write(&token_path, &json).await;
                    }
                    if let Some(tx) = event_tx.read().await.as_ref() {
                        let _ = tx.send(McpEvent::AuthComplete {
                            server_id: server_id_clone.clone(),
                        });
                    }
                }
                Err(e) => {
                    warn!("OAuth flow failed for '{server_id_clone}': {e}");
                    if let Some(tx) = event_tx.read().await.as_ref() {
                        let _ = tx.send(McpEvent::AuthFailed {
                            server_id: server_id_clone.clone(),
                            reason: e.to_string(),
                        });
                    }
                }
            }
            pending_auths.write().await.remove(&server_id_clone);
        });

        Ok(auth_url)
    }

    /// Cancel a pending OAuth authorization flow.
    pub async fn cancel_oauth(&self, server_id: &str) {
        if let Some(pending) = self.pending_auths.write().await.remove(server_id) {
            let _ = pending.cancel_tx.send(());
            self.emit_event(McpEvent::AuthFailed {
                server_id: server_id.to_string(),
                reason: "cancelled_by_user".to_string(),
            })
            .await;
        }
    }

    /// Extract the origin (scheme + host) from a URL.
    fn origin_from_url(url: &str) -> String {
        if let Some(rest) = url.strip_prefix("https://") {
            let end = rest.find('/').unwrap_or(rest.len());
            format!("https://{}", &rest[..end])
        } else if let Some(rest) = url.strip_prefix("http://") {
            let end = rest.find('/').unwrap_or(rest.len());
            format!("http://{}", &rest[..end])
        } else {
            url.to_string()
        }
    }

    /// Generate a PKCE code verifier (random bytes → base64url no-pad).
    fn generate_code_verifier() -> String {
        let mut bytes = vec![0u8; 64];
        OsRng.fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(&bytes)
    }

    /// Generate a PKCE code challenge (SHA-256 → base64url no-pad).
    fn generate_code_challenge(verifier: &str) -> String {
        let hash = Sha256::digest(verifier.as_bytes());
        URL_SAFE_NO_PAD.encode(&hash)
    }

    /// Generate a random state parameter.
    fn generate_random_state() -> String {
        let mut bytes = vec![0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(&bytes)
    }

    /// Run a mini HTTP server handling the OAuth redirect callback.
    async fn run_callback_server(
        listener: TcpListener,
        token_url: &str,
        code_verifier: &str,
        expected_state: &str,
        client_id: &str,
        redirect_uri: &str,
        cancel_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> crate::Result<OAuthTokens> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

        let accept = Box::pin(listener.accept());
        let cancel = Box::pin(cancel_rx);

        let (stream, _) = tokio::select! {
            result = accept => result.map_err(|e| {
                crate::error::SyscityError::Internal(format!("Callback server accept failed: {e}"))
            })?,
            _ = cancel => {
                return Err(crate::error::SyscityError::Internal(
                    "OAuth flow cancelled".to_string(),
                ));
            }
        };

        let (reader, mut writer) = tokio::io::split(stream);
        let mut lines = BufReader::new(reader).lines();

        let request_line = lines
            .next_line()
            .await
            .ok()
            .flatten()
            .unwrap_or_default();

        let path = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or("/");

        // Drain remaining request headers
        while let Ok(Some(line)) = lines.next_line().await {
            if line.is_empty() {
                break;
            }
        }

        // Parse query parameters
        let params: HashMap<String, String> = path
            .split('?')
            .nth(1)
            .unwrap_or("")
            .split('&')
            .filter_map(|pair| {
                let mut parts = pair.splitn(2, '=');
                let key = parts.next()?.to_string();
                let value = parts.next().unwrap_or("").to_string();
                Some((key, value))
            })
            .collect();

        let code = params
            .get("code")
            .ok_or_else(|| {
                crate::error::SyscityError::Internal(
                    "Missing code in OAuth callback".to_string(),
                )
            })?;

        let state = params
            .get("state")
            .ok_or_else(|| {
                crate::error::SyscityError::Internal(
                    "Missing state in OAuth callback".to_string(),
                )
            })?;

        if state != expected_state {
            let body = "Invalid state parameter. Authorization failed.";
            let _ = writer
                .write_all(
                    format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await;
            return Err(crate::error::SyscityError::Internal(
                "State mismatch in OAuth callback".to_string(),
            ));
        }

        // Exchange code for tokens
        let http_client = reqwest::Client::new();
        let token_body = format!(
            "grant_type=authorization_code&code={code}&redirect_uri={redirect_uri}&client_id={client_id}&code_verifier={code_verifier}"
        );

        let token_response = http_client
            .post(token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(token_body)
            .send()
            .await
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!("Token exchange failed: {e}"))
            })?;

        if !token_response.status().is_success() {
            let status = token_response.status();
            let error_text = token_response.text().await.unwrap_or_default();
            return Err(crate::error::SyscityError::Internal(format!(
                "Token exchange failed: HTTP {status} - {error_text}"
            )));
        }

        let token_data: serde_json::Value = token_response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to parse token response: {e}"))
        })?;

        let access_token = token_data["access_token"]
            .as_str()
            .ok_or_else(|| {
                crate::error::SyscityError::Internal(
                    "Missing access_token in token response".to_string(),
                )
            })?
            .to_string();

        let refresh_token = token_data["refresh_token"].as_str().map(String::from);
        let expires_at = token_data["expires_in"]
            .as_i64()
            .map(|secs| chrono::Utc::now().timestamp() + secs);

        let body = "<html><body><h1>Authorization complete!</h1><p>You may close this window and return to Syscity.</p></body></html>";
        let _ = writer
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await;

        Ok(OAuthTokens {
            access_token,
            refresh_token,
            expires_at,
        })
    }
}
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
        "mcp_connection"
    }

    fn description(&self) -> &str {
        r#"Connect to and use MCP (Model Context Protocol) servers.

Actions:
- connect: Connect to an MCP server
- disconnect: Disconnect from an MCP server
- list: List connected MCP servers
- tools: List available tools from a server
- resources: List resources available from a server
- resource_read: Read a resource by URI
- subscribe: Subscribe to resource change notifications
- unsubscribe: Unsubscribe from resource change notifications
- prompts: List available prompts from a server
- prompt_get: Render a prompt by name
- sampling: Create a sampling message through the server"#
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["connect", "disconnect", "list", "tools", "resources", "resource_read", "subscribe", "unsubscribe", "prompts", "prompt_get", "sampling"],
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
                    "description": "Resource URI (for resource_read / subscribe / unsubscribe)"
                },
                "prompt": {
                    "type": "string",
                    "description": "Prompt name (for prompt_get)"
                },
                "arguments": {
                    "type": "object",
                    "description": "Prompt arguments (for prompt_get)"
                },
                "messages": {
                    "type": "array",
                    "description": "Sampling messages (for sampling)"
                },
                "max_tokens": {
                    "type": "integer",
                    "description": "Maximum tokens for sampling (for sampling)",
                    "default": 1024
                },
                "model_hints": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Model hints for sampling (for sampling)"
                }
            },
            "required": ["action"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: true,
            risk_level: crate::tools::approval::RiskLevel::High,
            categories: vec!["network".to_string(), "mcp".to_string()],
            ..Default::default()
        }
    }

    fn is_available(&self, context: &ToolContext) -> bool {
        !context.sandboxed() || !context.allowed_commands().is_empty()
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let action = args["action"].as_str().ok_or_else(|| {
            crate::error::SyscityError::Validation("action is required".to_string())
        })?;

        match action {
            "connect" => {
                let server_id = args["server_id"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
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
                    crate::error::SyscityError::Validation(
                        "server_id is required for disconnect".to_string(),
                    )
                })?;
                if self.manager.get_client(server_id).await.is_none() {
                    return Ok(ToolExecutionResult::error(format!(
                        "MCP server '{}' is not connected",
                        server_id
                    )));
                }
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
                    crate::error::SyscityError::Validation(
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
                    crate::error::SyscityError::Validation(
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
                    crate::error::SyscityError::Validation(
                        "server_id is required for resource_read".to_string(),
                    )
                })?;
                let uri = args["uri"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
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

            "subscribe" => {
                let server_id = args["server_id"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "server_id is required for subscribe".to_string(),
                    )
                })?;
                let uri = args["uri"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "uri is required for subscribe".to_string(),
                    )
                })?;
                match self.manager.get_client(server_id).await {
                    Some(client_arc) => {
                        let client = client_arc.read().await;
                        client.subscribe_resource(uri).await?;
                        Ok(ToolExecutionResult::success(format!(
                            "Subscribed to resource updates for '{}' on '{}'",
                            uri, server_id
                        )))
                    }
                    None => Ok(ToolExecutionResult::error(format!(
                        "MCP server not found: {}",
                        server_id
                    ))),
                }
            }

            "unsubscribe" => {
                let server_id = args["server_id"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "server_id is required for unsubscribe".to_string(),
                    )
                })?;
                let uri = args["uri"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "uri is required for unsubscribe".to_string(),
                    )
                })?;
                match self.manager.get_client(server_id).await {
                    Some(client_arc) => {
                        let client = client_arc.read().await;
                        client.unsubscribe_resource(uri).await?;
                        Ok(ToolExecutionResult::success(format!(
                            "Unsubscribed from resource updates for '{}' on '{}'",
                            uri, server_id
                        )))
                    }
                    None => Ok(ToolExecutionResult::error(format!(
                        "MCP server not found: {}",
                        server_id
                    ))),
                }
            }

            "prompts" => {
                let server_id = args["server_id"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "server_id is required for prompts".to_string(),
                    )
                })?;
                match self.manager.get_client(server_id).await {
                    Some(client_arc) => {
                        let client = client_arc.read().await;
                        let prompts = client.list_prompts().await?;
                        Ok(ToolExecutionResult::success(format!(
                            "{} prompts from '{}'",
                            prompts.len(),
                            server_id
                        ))
                        .with_data(json!({ "prompts": prompts })))
                    }
                    None => Ok(ToolExecutionResult::error(format!(
                        "MCP server not found: {}",
                        server_id
                    ))),
                }
            }

            "prompt_get" => {
                let server_id = args["server_id"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "server_id is required for prompt_get".to_string(),
                    )
                })?;
                let prompt_name = args["prompt"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "prompt is required for prompt_get".to_string(),
                    )
                })?;
                let arguments = args["arguments"].as_object().map(|obj| {
                    obj.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                        .collect::<HashMap<_, _>>()
                });
                match self.manager.get_client(server_id).await {
                    Some(client_arc) => {
                        let client = client_arc.read().await;
                        let result = client.get_prompt(prompt_name, arguments).await?;
                        Ok(ToolExecutionResult::success(format!(
                            "Rendered prompt '{}' from '{}'",
                            prompt_name, server_id
                        ))
                        .with_data(json!(result)))
                    }
                    None => Ok(ToolExecutionResult::error(format!(
                        "MCP server not found: {}",
                        server_id
                    ))),
                }
            }

            "sampling" => {
                let server_id = args["server_id"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "server_id is required for sampling".to_string(),
                    )
                })?;
                let messages: Vec<McpSamplingMessage> =
                    serde_json::from_value(args["messages"].clone()).unwrap_or_default();
                let max_tokens = args["max_tokens"].as_i64().unwrap_or(1024);
                let model_hints = args["model_hints"].as_array().map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                });
                match self.manager.get_client(server_id).await {
                    Some(client_arc) => {
                        let client = client_arc.read().await;
                        let result = client
                            .sampling_create_message(messages, max_tokens, model_hints)
                            .await?;
                        Ok(ToolExecutionResult::success(format!(
                            "Sampling result from '{}'",
                            server_id
                        ))
                        .with_data(json!(result)))
                    }
                    None => Ok(ToolExecutionResult::error(format!(
                        "MCP server not found: {}",
                        server_id
                    ))),
                }
            }

            _ => Err(crate::error::SyscityError::Validation(format!("Unknown action: {}", action))),
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
        assert!(config.auto_reconnect);
        assert_eq!(config.health_check_interval_secs, 30);
        assert_eq!(config.max_reconnect_attempts, 5);
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
    fn test_server_capabilities_deserialization() {
        let caps: McpServerCapabilities = serde_json::from_value(json!({
            "tools": { "listChanged": true },
            "resources": { "subscribe": true, "listChanged": false },
            "prompts": { "listChanged": true }
        }))
        .unwrap();
        assert!(caps.supports_tools());
        assert!(caps.supports_tool_list_changed());
        assert!(caps.supports_resources());
        assert!(caps.supports_resource_subscribe());
        assert!(!caps.supports_resource_list_changed());
        assert!(caps.supports_prompts());
        assert!(caps.supports_prompt_list_changed());
    }

    #[test]
    fn test_initialize_result_deserialization() {
        let result: McpInitializeResult = serde_json::from_value(json!({
            "serverInfo": { "name": "test-server", "version": "1.0.0" },
            "capabilities": { "tools": {} }
        }))
        .unwrap();
        assert_eq!(result.server_info.name, "test-server");
        assert!(result.capabilities.supports_tools());
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
