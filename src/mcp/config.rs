//! MCP transport and server configuration types.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
#[derive(Clone, Serialize, Deserialize)]
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
    /// Literal env values applied verbatim at subprocess spawn (bypass `$VAR`
    /// expansion). Never serialized to config.toml — populated from the
    /// `~/.syscity/mcp_env` store at connect time.
    #[serde(default, skip)]
    pub resolved_env: HashMap<String, String>,
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

/// `Debug` redacts `resolved_env` values — they are secrets and must never
/// appear in logs.
impl std::fmt::Debug for McpServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted: HashMap<&String, &str> =
            self.resolved_env.keys().map(|k| (k, "••••")).collect();
        f.debug_struct("McpServerConfig")
            .field("transport", &self.transport)
            .field("command", &self.command)
            .field("args", &self.args)
            .field("env", &self.env)
            .field("resolved_env", &redacted)
            .field("working_dir", &self.working_dir)
            .field("url", &self.url)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_tools", &self.max_tools)
            .field("auto_connect", &self.auto_connect)
            .field("health_check_interval_secs", &self.health_check_interval_secs)
            .field("auto_reconnect", &self.auto_reconnect)
            .field("max_reconnect_attempts", &self.max_reconnect_attempts)
            .field("auth_type", &self.auth_type)
            .field("client_id", &self.client_id)
            .field("auth_url", &self.auth_url)
            .field("token_url", &self.token_url)
            .field("scopes", &self.scopes)
            .finish()
    }
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
            resolved_env: HashMap::new(),
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
