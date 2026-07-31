//! McpClient — one instance per connected MCP server.
//!
//! Handles JSON-RPC communication over stdio, SSE, or streamable-HTTP
//! transports.  Owns the server process watcher (stdio), request/response
//! channels, and the discovered tool/resource/prompt lists.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use futures::StreamExt;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{error, info, warn};

use crate::mcp::{
    McpGetPromptResult, McpInitializeResult, McpNotification, McpPrompt, McpRequest,
    McpResource, McpResourceContent, McpResponse, McpSamplingMessage, McpSamplingResult,
    McpServerCapabilities, McpServerConfig, McpServerInfo, McpToolDefinition, McpTransport,
};

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
    pub(crate) notification_tx: Option<mpsc::UnboundedSender<McpNotification>>,
    /// Broadcast channel for progress notifications used during streaming tool
    /// calls.
    pub(crate) progress_tx: Option<broadcast::Sender<McpNotification>>,
    /// Server metadata returned during `initialize`
    server_info: Option<McpServerInfo>,
    /// Server capabilities returned during `initialize`
    server_capabilities: Option<McpServerCapabilities>,
    /// Tools discovered via `tools/list`
    tools: Vec<McpToolDefinition>,
    /// Monotonically increasing JSON-RPC ID
    request_id: AtomicU64,
    /// Pending response channels keyed by request ID
    response_channels:
        Arc<RwLock<HashMap<u64, mpsc::UnboundedSender<McpResponse>>>>,
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
    pub(crate) fn resolve_env(
        env: &HashMap<String, String>,
    ) -> HashMap<String, String> {
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
    pub fn set_notification_tx(
        &mut self,
        tx: mpsc::UnboundedSender<McpNotification>,
    ) {
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
            "notifications/resources/list_changed" => {
                Some(McpNotification::ResourceListChanged)
            }
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
                Some(McpNotification::Message {
                    level,
                    data: params.clone(),
                })
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
    pub async fn connect_stdio(
        &mut self,
        config: McpServerConfig,
    ) -> crate::Result<()> {
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
            crate::error::SyscityError::Internal(format!(
                "Failed to spawn MCP server: {}",
                e
            ))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            crate::error::SyscityError::Internal("Failed to get stdin".to_string())
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            crate::error::SyscityError::Internal("Failed to get stdout".to_string())
        })?;

        let (request_tx, mut request_rx) =
            mpsc::unbounded_channel::<McpRequest>();
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
                        if let Some(notification) =
                            McpClient::parse_notification(&response)
                        {
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
    pub async fn connect_sse(
        &mut self,
        config: McpServerConfig,
    ) -> crate::Result<()> {
        let url = config.url.as_deref().ok_or_else(|| {
            crate::error::SyscityError::Internal(
                "SSE transport requires 'url' field".to_string(),
            )
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
                crate::error::SyscityError::Internal(format!(
                    "Failed to build HTTP client: {}",
                    e
                ))
            })?;

        // Channel for sending JSON-RPC requests to the writer task
        let (request_tx, mut request_rx) =
            mpsc::unbounded_channel::<McpRequest>();
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
            let mut builder =
                client.get(&get_url).header("Accept", "text/event-stream");
            for (k, v) in &env_headers {
                builder = builder.header(k, v);
            }
            if let Some(ref token) = access_token_sse {
                builder =
                    builder.header("Authorization", format!("Bearer {}", token));
            }
            let resp = match builder.send().await {
                Ok(r) => r,
                Err(e) => {
                    error!("SSE connection error: {}", e);
                    return;
                }
            };

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
                                    if let Ok(response) =
                                        serde_json::from_str::<McpResponse>(data)
                                    {
                                        if let Some(id) = response.id {
                                            let channels =
                                                response_channels_sse.read().await;
                                            if let Some(tx) = channels.get(&id) {
                                                let _ = tx.send(response);
                                            }
                                        } else {
                                            if let Some(notification) =
                                                McpClient::parse_notification(
                                                    &response,
                                                )
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
                crate::error::SyscityError::Internal(format!(
                    "Failed to build HTTP client: {}",
                    e
                ))
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
                    builder =
                        builder.header("Authorization", format!("Bearer {}", token));
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
    pub async fn connect_streamable_http(
        &mut self,
        config: McpServerConfig,
    ) -> crate::Result<()> {
        let url = config.url.as_deref().ok_or_else(|| {
            crate::error::SyscityError::Internal(
                "streamable_http transport requires 'url' field".to_string(),
            )
        })?;

        info!(
            "Connecting to MCP server via streamable-HTTP: {}",
            url
        );

        self.timeout_secs = config.timeout_secs;
        self.server_config = Some(config.clone());

        let resolved_env = Self::resolve_env(&config.env);

        let (request_tx, mut request_rx) =
            mpsc::unbounded_channel::<McpRequest>();
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
                    builder =
                        builder.header("Authorization", format!("Bearer {}", token));
                }

                let resp = match builder.send().await {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Failed to POST MCP request: {}", e);
                        continue;
                    }
                };

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
                                    if let Some(data) =
                                        line.strip_prefix("data:")
                                    {
                                        let data = data.trim();
                                        if let Ok(response) =
                                            serde_json::from_str::<McpResponse>(
                                                data,
                                            )
                                        {
                                            if let Some(id) = response.id {
                                                let channels =
                                                    response_channels.read().await;
                                                if let Some(tx) =
                                                    channels.get(&id)
                                                {
                                                    let _ = tx.send(response);
                                                }
                                            } else {
                                                if let Some(notification) =
                                                    McpClient::parse_notification(
                                                        &response,
                                                    )
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
    pub async fn connect(
        &mut self,
        config: McpServerConfig,
    ) -> crate::Result<()> {
        match config.transport {
            McpTransport::Stdio => self.connect_stdio(config).await,
            McpTransport::Sse => self.connect_sse(config).await,
            McpTransport::StreamableHttp => {
                self.connect_streamable_http(config).await
            }
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
            if let Ok(init_result) =
                serde_json::from_value::<McpInitializeResult>(result)
            {
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
            info!(
                "MCP server does not advertise tool support; skipping tools/list"
            );
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
    async fn send_request(
        &self,
        request: McpRequest,
    ) -> crate::Result<McpResponse> {
        let id = request.id.unwrap_or(0);

        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let mut channels = self.response_channels.write().await;
            channels.insert(id, tx);
        }

        if let Some(ref req_tx) = self.request_tx {
            req_tx.send(request).map_err(|_| {
                crate::error::SyscityError::Internal(
                    "Request channel closed".to_string(),
                )
            })?;
        } else {
            return Err(crate::error::SyscityError::Internal(
                "Not connected".to_string(),
            ));
        }

        let timeout = tokio::time::Duration::from_secs(self.timeout_secs);
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(response)) => {
                let mut channels = self.response_channels.write().await;
                channels.remove(&id);

                if let Some(error) = response.error {
                    return Err(crate::error::SyscityError::ExternalService {
                        source: format!(
                            "MCP error {}: {}",
                            error.code, error.message
                        ),
                        cause: None,
                    });
                }

                Ok(response)
            }
            Ok(None) => {
                Err(crate::error::SyscityError::Internal(
                    "Response channel closed".to_string(),
                ))
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
            let tools: Vec<McpToolDefinition> =
                if let Some(arr) = result.get("tools") {
                    serde_json::from_value::<Vec<McpToolDefinition>>(arr.clone()).unwrap_or_default()
                } else {
                    serde_json::from_value::<Vec<McpToolDefinition>>(result).unwrap_or_default()
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
            crate::error::SyscityError::Internal(
                "No result from tool call".to_string(),
            )
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
            let prompts: Vec<McpPrompt> =
                if let Some(arr) = result.get("prompts") {
                    serde_json::from_value::<Vec<McpPrompt>>(arr.clone()).unwrap_or_default()
                } else {
                    serde_json::from_value::<Vec<McpPrompt>>(result).unwrap_or_default()
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
            crate::error::SyscityError::Internal(
                "No result from prompts/get".to_string(),
            )
        })?;
        serde_json::from_value::<McpGetPromptResult>(result).map_err(|e| {
            crate::error::SyscityError::Internal(format!(
                "Failed to parse prompt result: {}",
                e
            ))
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
                obj.insert(
                    "modelPreferences".to_string(),
                    json!({ "hints": hints }),
                );
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
            crate::error::SyscityError::Internal(format!(
                "Failed to parse sampling result: {}",
                e
            ))
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
            obj.insert(
                "_meta".to_string(),
                json!({ "progressToken": progress_token }),
            );
        }

        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "tools/call".to_string(),
            params: Some(request_params),
        };

        let response = self.send_request(request).await?;
        response.result.ok_or_else(|| {
            crate::error::SyscityError::Internal(
                "No result from tool call".to_string(),
            )
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
            let resources: Vec<McpResource> =
                if let Some(arr) = result.get("resources") {
                    serde_json::from_value::<Vec<McpResource>>(arr.clone()).unwrap_or_default()
                } else {
                    serde_json::from_value::<Vec<McpResource>>(result).unwrap_or_default()
                };
            return Ok(resources);
        }
        Ok(Vec::new())
    }

    /// Read a resource by URI.
    pub async fn read_resource(
        &self,
        uri: &str,
    ) -> crate::Result<Vec<McpResourceContent>> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "resources/read".to_string(),
            params: Some(json!({ "uri": uri })),
        };

        let response = self.send_request(request).await?;
        if let Some(result) = response.result {
            let contents: Vec<McpResourceContent> =
                if let Some(arr) = result.get("contents") {
                    serde_json::from_value::<Vec<McpResourceContent>>(arr.clone()).unwrap_or_default()
                } else {
                    serde_json::from_value::<Vec<McpResourceContent>>(result).unwrap_or_default()
                };
            return Ok(contents);
        }
        Ok(Vec::new())
    }

    /// Subscribe to resource change notifications for a URI.
    pub async fn subscribe_resource(
        &self,
        uri: &str,
    ) -> crate::Result<()> {
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
    pub async fn unsubscribe_resource(
        &self,
        uri: &str,
    ) -> crate::Result<()> {
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
    pub fn server_capabilities(
        &self,
    ) -> Option<&McpServerCapabilities> {
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
        self.request_tx.is_some()
            && !self.child_exited.load(Ordering::Relaxed)
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
