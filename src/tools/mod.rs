//! Tool abstractions for Syscity
//!
//! Tools are capabilities that the AI assistant can use to interact
//! with the world (execute shell commands, read files, search the web, etc.).

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_stream::StreamExt;
use tracing::warn;

use crate::providers::{FunctionCall, FunctionDefinition, ToolResult};

pub mod approval;
pub mod rbac;

// Re-export approval types for convenience
pub use approval::{
    ApprovalDecision, ApprovalFilter, ApprovalLevel, ApprovalQueue, ApprovalRequiredEvent,
    PendingApproval, PendingApprovalSummary, RiskLevel,
};
// Re-export RBAC types for convenience
pub use rbac::{
    ModelCapabilities, PolicyEvaluationContext, Role, SandboxPolicy, ToolPolicy, UserContext,
};

/// Skill trust level for tool access control.
///
/// The minimum trust across all active skills constrains the available
/// tool set — a community skill mixed with a trusted one does not escalate
/// privileges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillTrust {
    /// Community / untrusted skill — read-only (non-privileged) tools only.
    Community = 0,
    /// Installed / trusted skill — full tool access.
    #[default]
    Trusted = 1,
}

/// A unique identifier for a tool
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolId(pub String);

impl ToolId {
    /// Create a new tool ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for ToolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identity fields: who is calling the tool.
#[derive(Debug, Clone, Default)]
pub struct ToolIdentity {
    /// The user ID executing the tool
    pub user_id: String,
    /// The conversation ID
    pub conversation_id: String,
    /// Identifier of the sender/user that triggered the invocation.
    pub sender_id: Option<String>,
    /// Whether the sender is the system owner.
    pub sender_is_owner: bool,
    /// Optional per-user RBAC context.
    pub user_context: Option<UserContext>,
}

/// Sandbox / execution environment: where and how the tool runs.
#[derive(Debug, Clone)]
pub struct ToolSandbox {
    /// The working directory for file operations
    pub working_directory: std::path::PathBuf,
    /// Environment variables
    pub environment: HashMap<String, String>,
    /// Timeout for tool execution
    pub timeout: Duration,
    /// Allowed paths for file operations (if empty, no restrictions)
    pub allowed_paths: Vec<std::path::PathBuf>,
    /// Allowed commands for shell execution (if empty, no restrictions)
    pub allowed_commands: Vec<String>,
    /// Whether the tool is being executed in a sandbox
    pub sandboxed: bool,
    /// Maximum memory allowed for child processes in bytes (if sandboxed)
    pub memory_limit: Option<usize>,
    /// Maximum CPU time in seconds (if sandboxed)
    pub cpu_limit: Option<u64>,
    /// Maximum number of open file descriptors
    pub fd_limit: Option<u64>,
    /// Maximum process count (for preventing fork bombs)
    pub process_limit: Option<u64>,
    /// Root directory for file operations (workspace boundary).
    pub workspace_root: std::path::PathBuf,
    /// When true, file operations are restricted to `workspace_root`.
    pub workspace_only: bool,
    /// Optional sandbox policy applied to tool execution.
    pub sandbox_policy: Option<SandboxPolicy>,
    /// Optional allowlist of plugin tool prefixes/names.
    pub plugin_allowlist: Option<Vec<String>>,
}

impl Default for ToolSandbox {
    fn default() -> Self {
        Self {
            working_directory: std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from(".")),
            environment: default_tool_environment(),
            timeout: Duration::from_secs(30),
            allowed_paths: Vec::new(),
            allowed_commands: Vec::new(),
            sandboxed: false,
            memory_limit: None,
            cpu_limit: None,
            fd_limit: None,
            process_limit: None,
            workspace_root: crate::dirs::workspace_data_dir(),
            workspace_only: true,
            sandbox_policy: None,
            plugin_allowlist: None,
        }
    }
}

/// Model / policy metadata: capabilities and access control.
#[derive(Debug, Clone)]
pub struct ToolModel {
    /// Name of the LLM model driving the current invocation.
    pub model_name: Option<String>,
    /// Name of the LLM provider driving the current invocation.
    pub provider_name: Option<String>,
    /// Capabilities of the current model (vision, tool use, etc.).
    pub model_capabilities: ModelCapabilities,
    /// Minimum trust level from active skills.
    pub skill_trust: SkillTrust,
    /// Optional per-context RBAC policy.
    pub tool_policy: Option<ToolPolicy>,
}

impl Default for ToolModel {
    fn default() -> Self {
        Self {
            model_name: None,
            provider_name: None,
            model_capabilities: ModelCapabilities::default(),
            skill_trust: SkillTrust::Trusted,
            tool_policy: None,
        }
    }
}

/// The execution context for a tool
///
/// Groups fields into three sub-structs for construction ergonomics:
/// - `identity`: who is making the call (`user_id`, `conversation_id`, etc.)
/// - `sandbox`: execution environment (working directory, timeouts, limits)
/// - `model`: LLM / policy metadata (model name, capabilities, trust)
///
/// Common identity fields are accessible via `Deref` — `ctx.user_id` still
/// works. Sandbox fields have convenience accessors: `ctx.working_directory()`.
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    /// Identity fields (who is calling the tool)
    pub identity: ToolIdentity,
    /// Sandbox / execution environment
    pub sandbox: ToolSandbox,
    /// Model / policy metadata
    pub model: ToolModel,
}

/// Allowed environment variables that are safe to forward to child processes.
///
/// This whitelist avoids leaking secrets such as API keys into shell and other
/// subprocess executions while preserving common system variables required by
/// most programs.
const ALLOWED_ENV_VARS: &[&str] = &[
    "PATH", "HOME", "USER", "SHELL", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR", "TERM", "TZ",
];

/// Build a whitelisted environment map from the current process environment.
fn default_tool_environment() -> HashMap<String, String> {
    std::env::vars()
        .filter(|(k, _)| ALLOWED_ENV_VARS.contains(&k.as_str()))
        .collect()
}

impl std::ops::Deref for ToolContext {
    type Target = ToolIdentity;
    fn deref(&self) -> &Self::Target {
        &self.identity
    }
}

impl ToolContext {
    /// Convenience accessors for commonly-used sandbox fields.
    pub fn working_directory(&self) -> &std::path::PathBuf {
        &self.sandbox.working_directory
    }
    pub fn environment(&self) -> &HashMap<String, String> {
        &self.sandbox.environment
    }
    pub fn timeout(&self) -> Duration {
        self.sandbox.timeout
    }
    pub fn sandboxed(&self) -> bool {
        self.sandbox.sandboxed
    }
    pub fn allowed_paths(&self) -> &[std::path::PathBuf] {
        &self.sandbox.allowed_paths
    }
    pub fn allowed_commands(&self) -> &[String] {
        &self.sandbox.allowed_commands
    }
    pub fn workspace_root(&self) -> &std::path::PathBuf {
        &self.sandbox.workspace_root
    }
    pub fn workspace_only(&self) -> bool {
        self.sandbox.workspace_only
    }
    pub fn memory_limit(&self) -> Option<usize> {
        self.sandbox.memory_limit
    }
    pub fn cpu_limit(&self) -> Option<u64> {
        self.sandbox.cpu_limit
    }
    pub fn fd_limit(&self) -> Option<u64> {
        self.sandbox.fd_limit
    }
    pub fn process_limit(&self) -> Option<u64> {
        self.sandbox.process_limit
    }
    pub fn sandbox_policy(&self) -> Option<&SandboxPolicy> {
        self.sandbox.sandbox_policy.as_ref()
    }
    pub fn plugin_allowlist(&self) -> Option<&[String]> {
        self.sandbox.plugin_allowlist.as_deref()
    }

    /// Create a new tool context
    pub fn new(user_id: impl Into<String>, conversation_id: impl Into<String>) -> Self {
        Self {
            identity: ToolIdentity {
                user_id: user_id.into(),
                conversation_id: conversation_id.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Set the workspace root directory
    pub fn with_workspace_root(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.sandbox.workspace_root = path.into();
        self
    }

    /// Set workspace-only mode (restrict file ops to workspace_root)
    pub fn with_workspace_only(mut self, enabled: bool) -> Self {
        self.sandbox.workspace_only = enabled;
        self
    }

    /// Set the minimum skill trust level (controls which tools are exposed).
    pub fn with_skill_trust(mut self, trust: SkillTrust) -> Self {
        self.model.skill_trust = trust;
        self
    }

    /// Set the RBAC user context for policy evaluation.
    pub fn with_user_context(mut self, ctx: UserContext) -> Self {
        self.identity.user_context = Some(ctx);
        self
    }

    /// Set the RBAC policy applied to this context.
    pub fn with_tool_policy(mut self, policy: ToolPolicy) -> Self {
        self.model.tool_policy = Some(policy);
        self
    }

    /// Set the model name for model-based tool gating.
    pub fn with_model_name(mut self, model: impl Into<String>) -> Self {
        self.model.model_name = Some(model.into());
        self
    }

    /// Set the provider name for provider-based tool gating.
    pub fn with_provider_name(mut self, provider: impl Into<String>) -> Self {
        self.model.provider_name = Some(provider.into());
        self
    }

    /// Set the sender identifier for sender-based tool gating.
    pub fn with_sender_id(mut self, sender: impl Into<String>) -> Self {
        self.identity.sender_id = Some(sender.into());
        self
    }

    /// Mark the sender as the system owner.
    pub fn with_sender_is_owner(mut self, is_owner: bool) -> Self {
        self.identity.sender_is_owner = is_owner;
        self
    }

    /// Set an allowlist of plugin tool prefixes/names.
    pub fn with_plugin_allowlist(mut self, allowlist: Vec<String>) -> Self {
        self.sandbox.plugin_allowlist = Some(allowlist);
        self
    }

    /// Set the model capabilities for model-based tool gating.
    pub fn with_model_capabilities(mut self, capabilities: ModelCapabilities) -> Self {
        self.model.model_capabilities = capabilities;
        self
    }

    /// Set the sandbox policy applied to tool execution.
    pub fn with_sandbox_policy(mut self, policy: SandboxPolicy) -> Self {
        self.sandbox.sandbox_policy = Some(policy);
        self
    }

    /// Set the working directory
    pub fn with_working_dir(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.sandbox.working_directory = path.into();
        self
    }

    /// Set the timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.sandbox.timeout = timeout;
        self
    }

    /// Add an allowed path
    pub fn allow_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.sandbox.allowed_paths.push(path.into());
        self
    }

    /// Add an allowed command
    pub fn allow_command(mut self, command: impl Into<String>) -> Self {
        self.sandbox.allowed_commands.push(command.into());
        self
    }

    /// Set sandboxed mode
    pub fn with_sandboxed(mut self, sandboxed: bool) -> Self {
        self.sandbox.sandboxed = sandboxed;
        self
    }

    /// Set memory limit in bytes (only effective when sandboxed)
    pub fn with_memory_limit(mut self, bytes: usize) -> Self {
        self.sandbox.memory_limit = Some(bytes);
        self
    }

    /// Set CPU time limit in seconds (only effective when sandboxed)
    pub fn with_cpu_limit(mut self, seconds: u64) -> Self {
        self.sandbox.cpu_limit = Some(seconds);
        self
    }

    /// Set file descriptor limit (only effective when sandboxed)
    pub fn with_fd_limit(mut self, count: u64) -> Self {
        self.sandbox.fd_limit = Some(count);
        self
    }

    /// Set process limit for preventing fork bombs (only effective when
    /// sandboxed)
    pub fn with_process_limit(mut self, count: u64) -> Self {
        self.sandbox.process_limit = Some(count);
        self
    }

    /// Apply resource limits to the current process (Unix only)
    /// This should be called in a pre_exec hook before spawning the child
    /// process
    #[cfg(unix)]
    pub fn apply_resource_limits(&self) -> std::io::Result<()> {
        use std::io;

        // Only apply limits if sandboxed
        if !self.sandbox.sandboxed {
            return Ok(());
        }

        // Apply memory limit
        if let Some(memory_limit) = self.sandbox.memory_limit {
            // SAFETY: setrlimit is a standard POSIX syscall that modifies resource
            // limits for the current process. It is async-signal-safe and does not
            // access invalid memory.
            #[allow(unsafe_code)]
            unsafe {
                let limit = libc::rlimit {
                    rlim_cur: memory_limit as libc::rlim_t,
                    rlim_max: memory_limit as libc::rlim_t,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }

        // Apply CPU limit
        if let Some(cpu_limit) = self.sandbox.cpu_limit {
            // SAFETY: setrlimit is a standard POSIX syscall that modifies resource
            // limits for the current process. It is async-signal-safe and does not
            // access invalid memory.
            #[allow(unsafe_code)]
            unsafe {
                let limit = libc::rlimit {
                    rlim_cur: cpu_limit as libc::rlim_t,
                    rlim_max: cpu_limit as libc::rlim_t,
                };
                if libc::setrlimit(libc::RLIMIT_CPU, &limit) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }

        // Apply file descriptor limit
        if let Some(fd_limit) = self.sandbox.fd_limit {
            // SAFETY: setrlimit is a standard POSIX syscall that modifies resource
            // limits for the current process. It is async-signal-safe and does not
            // access invalid memory.
            #[allow(unsafe_code)]
            unsafe {
                let limit = libc::rlimit {
                    rlim_cur: fd_limit as libc::rlim_t,
                    rlim_max: fd_limit as libc::rlim_t,
                };
                if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }

        // Apply process limit (NPROC)
        if let Some(process_limit) = self.sandbox.process_limit {
            // SAFETY: setrlimit is a standard POSIX syscall that modifies resource
            // limits for the current process. It is async-signal-safe and does not
            // access invalid memory.
            #[allow(unsafe_code)]
            unsafe {
                let limit = libc::rlimit {
                    rlim_cur: process_limit as libc::rlim_t,
                    rlim_max: process_limit as libc::rlim_t,
                };
                if libc::setrlimit(libc::RLIMIT_NPROC, &limit) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }

        Ok(())
    }

    /// Apply resource limits is a no-op on non-Unix platforms
    #[cfg(not(unix))]
    pub fn apply_resource_limits(&self) -> std::io::Result<()> {
        // Resource limits are not implemented for non-Unix platforms
        Ok(())
    }

    /// Get a human-readable summary of resource limits
    pub fn resource_limits_summary(&self) -> String {
        if !self.sandbox.sandboxed {
            return "No sandbox (no resource limits)".to_string();
        }

        let mut parts = vec!["Sandbox active".to_string()];

        if let Some(mem) = self.sandbox.memory_limit {
            parts.push(format!("Memory: {} MB", mem / 1024 / 1024));
        }
        if let Some(cpu) = self.sandbox.cpu_limit {
            parts.push(format!("CPU: {}s", cpu));
        }
        if let Some(fd) = self.sandbox.fd_limit {
            parts.push(format!("FDs: {}", fd));
        }
        if let Some(proc) = self.sandbox.process_limit {
            parts.push(format!("Processes: {}", proc));
        }

        if parts.len() == 1 {
            parts.push("No specific limits set".to_string());
        }

        parts.join(" | ")
    }

    /// Check if a path is allowed
    pub fn is_path_allowed(&self, path: &std::path::Path) -> bool {
        // ── allowlist check ────────────────────────────────────────────────
        if !self.sandbox.allowed_paths.is_empty() {
            let path_canon = path.canonicalize().ok();
            let path_raw = path.to_path_buf();
            let in_allowlist = self.sandbox.allowed_paths.iter().any(|allowed| {
                // Try canonical comparison first (handles symlinks)
                if let Ok(ref ac) = allowed.canonicalize() {
                    if let Some(ref pc) = path_canon {
                        if pc.starts_with(ac) {
                            return true;
                        }
                    }
                }
                // Fallback to raw path comparison for non-existent paths
                path_raw.starts_with(allowed)
            });
            // When an allowlist is present it acts as a whitelist:
            // paths inside the allowlist are permitted, everything else is denied.
            return in_allowlist;
        }

        // ── workspace boundary check ──────────────────────
        if self.sandbox.workspace_only {
            let resolved = self.resolve_path(path);
            let resolved_canon = resolved.canonicalize().ok();
            let root_canon = self.sandbox.workspace_root.canonicalize().ok();

            let within = if let (Some(ref rc), Some(ref wc)) = (resolved_canon, root_canon) {
                rc.starts_with(wc)
            } else {
                resolved.starts_with(&self.sandbox.workspace_root)
            };

            if !within {
                return false;
            }
        }

        true
    }

    /// Resolve a path relative to the workspace root.
    ///
    /// * Absolute paths are returned as-is (but still subject to
    ///   `is_path_allowed`).
    /// * Relative paths are joined with `workspace_root`.
    /// * `~` is expanded to the user's home directory.
    pub fn resolve_path(&self, path: &std::path::Path) -> std::path::PathBuf {
        // Expand tilde
        let expanded = if let Some(path_str) = path.to_str() {
            if path_str.starts_with("~/") || path_str == "~" {
                if let Some(home) = dirs::home_dir() {
                    let rest = &path_str[1..];
                    home.join(rest.trim_start_matches('/'))
                } else {
                    path.to_path_buf()
                }
            } else {
                path.to_path_buf()
            }
        } else {
            path.to_path_buf()
        };

        if expanded.is_absolute() {
            expanded
        } else {
            self.sandbox.workspace_root.join(expanded)
        }
    }

    /// Check if a command is allowed
    pub fn is_command_allowed(&self, command: &str) -> bool {
        if self.sandbox.allowed_commands.is_empty() {
            return true;
        }
        let cmd = command.split_whitespace().next().unwrap_or(command);
        self.sandbox
            .allowed_commands
            .iter()
            .any(|allowed| allowed == cmd)
    }
}

/// A chunk emitted by a streaming tool execution.
///
/// Streaming tools can emit output, errors, and structured data incrementally
/// instead of buffering everything into a single [`ToolExecutionResult`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "payload")]
pub enum ToolExecutionChunk {
    /// Standard output / progress text.
    Output(String),
    /// Standard error / error text.
    Error(String),
    /// Structured data chunk.
    Data(Value),
    /// Stream completed successfully.
    Done,
}

/// The result of a tool execution
#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    /// Whether the execution was successful
    pub success: bool,
    /// The output data
    pub output: String,
    /// Error message if failed
    pub error: Option<String>,
    /// Additional structured data
    pub data: Option<Value>,
    /// Execution time
    pub execution_time: Duration,
}

impl Serialize for ToolExecutionResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ToolExecutionResult", 5)?;
        state.serialize_field("success", &self.success)?;
        state.serialize_field("output", &self.output)?;
        state.serialize_field("error", &self.error)?;
        state.serialize_field("data", &self.data)?;
        state.serialize_field("execution_time", &self.execution_time.as_millis())?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for ToolExecutionResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            success: bool,
            output: String,
            error: Option<String>,
            data: Option<Value>,
            execution_time: u64,
        }

        let helper = Helper::deserialize(deserializer)?;
        Ok(ToolExecutionResult {
            success: helper.success,
            output: helper.output,
            error: helper.error,
            data: helper.data,
            execution_time: Duration::from_millis(helper.execution_time),
        })
    }
}

impl std::fmt::Display for ToolExecutionResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.output)
    }
}

impl ToolExecutionResult {
    /// Create a successful result
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            error: None,
            data: None,
            execution_time: Duration::default(),
        }
    }

    /// Create an error result
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error.into()),
            data: None,
            execution_time: Duration::default(),
        }
    }

    /// Add structured data
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Set execution time
    pub fn with_execution_time(mut self, duration: Duration) -> Self {
        self.execution_time = duration;
        self
    }

    /// Returns `true` if this is a successful result with no output content.
    pub fn is_empty(&self) -> bool {
        self.success && self.output.is_empty() && self.error.is_none()
    }

    /// Returns `true` if this result represents a failure.
    pub fn is_error(&self) -> bool {
        !self.success
    }

    /// Convert to a ToolResult for LLM response
    pub fn to_tool_result(self, tool_call_id: impl Into<String>) -> ToolResult {
        let content = if self.success {
            self.output
        } else {
            format!("Error: {}", self.error.unwrap_or_else(|| "Unknown error".to_string()))
        };

        ToolResult {
            tool_call_id: tool_call_id.into(),
            role: crate::providers::Role::Tool,
            content,
            is_error: Some(!self.success),
        }
    }
}

/// Trait for tools that can be executed by the agent
#[async_trait]
pub trait Tool: Send + Sync + 'static {
    /// Get the unique name of this tool
    fn name(&self) -> &str;

    /// Get a description of what this tool does
    fn description(&self) -> &str;

    /// Get the JSON schema for this tool's parameters
    fn parameters_schema(&self) -> Value;

    /// Execute the tool with the given arguments
    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult>;

    /// Execute the tool as a stream of incremental chunks.
    ///
    /// The default implementation calls [`execute`](Tool::execute) once and
    /// yields the resulting output/error/data as a single chunk sequence.
    /// Tools that produce incremental output (long-running shells, process
    /// runners, etc.) should override this to emit [`ToolExecutionChunk`]s
    /// as data becomes available.
    fn execute_stream<'a>(
        &'a self,
        args: Value,
        context: &'a ToolContext,
    ) -> Pin<Box<dyn tokio_stream::Stream<Item = ToolExecutionChunk> + Send + 'a>> {
        Box::pin(async_stream::stream! {
            match self.execute(args, context).await {
                Ok(result) => {
                    if !result.output.is_empty() {
                        yield ToolExecutionChunk::Output(result.output);
                    }
                    if let Some(error) = result.error {
                        yield ToolExecutionChunk::Error(error);
                    }
                    if let Some(data) = result.data {
                        yield ToolExecutionChunk::Data(data);
                    }
                }
                Err(e) => yield ToolExecutionChunk::Error(e.to_string()),
            }
        })
    }

    /// Check if this tool is available in the given context
    fn is_available(&self, _context: &ToolContext) -> bool {
        true
    }

    /// Advertised capabilities for RBAC and SDK discovery.
    ///
    /// Defaults to a low-risk, uncategorized tool. Individual tools can
    /// override this to expose their real risk level and categories.
    fn capabilities(&self) -> crate::tools::sdk::ToolCapabilities {
        crate::tools::sdk::ToolCapabilities::default()
    }

    /// Get the timeout for this tool (defaults to context timeout)
    fn timeout(&self, context: &ToolContext) -> Duration {
        context.timeout()
    }

    /// Convert to a function definition for LLM providers
    fn to_function_definition(&self) -> FunctionDefinition {
        FunctionDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }

}

/// A boxed tool for storage
pub type BoxedTool = Box<dyn Tool>;
/// An atomically-reference-counted tool for shared storage
pub type SharedTool = Arc<dyn Tool>;

pub mod acp_tool;
pub mod agents_list;
pub mod browser;
pub mod canvas;
pub mod code_exec;
pub mod computer;
pub mod command_detector;
pub mod command_gate;
pub mod cron_tool;
pub mod delegate_tool;
pub mod planner;
pub mod file;
pub mod gateway;
pub mod grep;
pub mod heartbeat_tool;
pub mod hooks;
pub mod image;
pub mod list_capabilities;
pub mod mcp;
pub mod memory;
pub mod message;
pub mod nodes;
pub mod patch;
pub mod pdf;
pub mod process;
pub mod sandbox;
pub mod sandbox_interceptor;
pub mod sdk;
pub mod session;
pub mod shell;
pub mod shell_safety;
pub mod stt;
pub mod time;
pub mod todo_tool;
pub mod tts;
pub mod update_plan;
pub mod web;

pub use acp_tool::{AcpSessionTool, AcpSpawnTool};
pub use agents_list::AgentsListTool;
pub use browser::BrowserTool;
pub use canvas::CanvasTool;
pub use code_exec::CodeExecutionTool;
pub use cron_tool::CronTool;
pub use delegate_tool::{AgentResolver, DelegateTool};
pub use file::{FileEditTool, FileReadTool, FileWriteTool, GlobTool};
pub use gateway::GatewayTool;
pub use grep::GrepTool;
pub use heartbeat_tool::HeartbeatTool;
pub use hooks::{ToolHooks, ToolPolicyDecision};
pub use image::{ImageGenerateTool, ImageTool};
pub use list_capabilities::ListCapabilitiesTool;
pub use mcp::McpConnectionTool;
pub use memory::{MemoryGetTool, MemorySearchTool, MemoryTool};
pub use message::MessageTool;
pub use nodes::NodesTool;
pub use patch::ApplyPatchTool;
pub use pdf::PdfTool;
pub use process::ProcessTool;
pub use sandbox::{SandboxConfig, SandboxedTool};
pub use sdk::{
    CapabilityFilter, SyncResult, ToolCapabilities, ToolMetadata, ToolPack, ToolSdk, ToolSdkError,
};
pub use session::{
    SessionStatusTool, SessionsHistoryTool, SessionsListTool, SessionsSendTool, SessionsYieldTool,
};
pub use shell::ShellTool;
pub use stt::SttTool;
pub use time::TimeTool;
pub use todo_tool::TodoTool;
pub use tts::TtsTool;
pub use update_plan::UpdatePlanTool;
pub use web::{WebFetchTool, WebSearchTool};

/// Cached tool result entry
#[derive(Debug, Clone)]
struct CacheEntry {
    result: ToolExecutionResult,
    timestamp: std::time::Instant,
}

/// Shared, mutable list of web search providers. Held by both WebSearchTool
/// and ToolRegistry so hot-reload can update providers without rebuilding
/// the registry.
pub type WebSearchProviders = std::sync::Arc<tokio::sync::RwLock<Vec<crate::tools::web::SearchProvider>>>;

/// Registry of tools with optional caching, circuit breaker, and trust-level
/// filtering.
pub struct ToolRegistry {
    tools: std::sync::RwLock<HashMap<String, SharedTool>>,
    /// Dynamically registered tools (e.g. MCP auto-discovered tools).
    /// Uses interior mutability so tools can be added through
    /// `Arc<ToolRegistry>`.
    dynamic_tools: std::sync::RwLock<HashMap<String, std::sync::Arc<dyn Tool>>>,
    /// Tool-name prefixes that have been logically deregistered (e.g. MCP
    /// server disconnect). Tools matching any blocked prefix are excluded
    /// from `get`, `list`, `has`, `get_definitions`, and `get_available`
    /// without requiring `&mut self` — allowing this to be called through an
    /// `Arc<ToolRegistry>`.
    blocked_prefixes: std::sync::RwLock<HashSet<String>>,
    cache: std::sync::Mutex<HashMap<String, CacheEntry>>,
    cache_ttl: Option<Duration>,
    cache_enabled: bool,
    /// Per-tool failure counts for circuit breaker logic.
    failure_counts: std::sync::RwLock<HashMap<String, u32>>,
    /// Tool names that require `SkillTrust::Trusted` access.
    /// When a context has `skill_trust == Community` these tools are hidden.
    privileged_tools: std::sync::RwLock<HashSet<String>>,
    /// Hooks for tool execution (before/after/policy).
    hooks: ToolHooks,
    /// Runtime-override hooks (set through `&self` via `set_hooks`).
    /// Allows tests to inject policy hooks through an `Arc<ToolRegistry>`.
    /// Takes precedence over `self.hooks` when `Some`.
    hooks_override: std::sync::Mutex<Option<ToolHooks>>,
    /// Approval queue for human-in-the-loop tool execution.
    /// When set, high-risk tool calls can be suspended pending human approval.
    approval_queue: Option<Arc<ApprovalQueue>>,
    /// Content filter for scanning tool outputs for PII and secrets.
    content_filter: Option<Arc<crate::security::content_filter::ContentFilter>>,
    /// Audit logger for recording tool invocations and security events.
    audit_log: Option<Arc<dyn crate::security::runtime_audit::AuditLogger>>,
    /// Shared provider list for the web_search tool. Hot-reload updates this
    /// directly when `[search]` configuration changes.
    web_search_providers: Option<WebSearchProviders>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            tools: std::sync::RwLock::new(HashMap::new()),
            dynamic_tools: std::sync::RwLock::new(HashMap::new()),
            blocked_prefixes: std::sync::RwLock::new(HashSet::new()),
            cache: std::sync::Mutex::new(HashMap::new()),
            cache_ttl: None,
            cache_enabled: true,
            failure_counts: std::sync::RwLock::new(HashMap::new()),
            privileged_tools: std::sync::RwLock::new(HashSet::new()),
            hooks: ToolHooks::new(),
            hooks_override: std::sync::Mutex::new(None),
            approval_queue: None,
            content_filter: None,
            audit_log: None,
            web_search_providers: None,
        }
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field(
                "tools",
                &self
                    .tools
                    .read()
                    .map(|m| m.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default(),
            )
            .field("hooks", &self.hooks)
            .field("approval_queue", &self.approval_queue.is_some())
            .finish()
    }
}

impl ToolRegistry {
    /// Number of consecutive failures before a tool is circuit-broken.
    pub const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;

    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            tools: std::sync::RwLock::new(HashMap::new()),
            dynamic_tools: std::sync::RwLock::new(HashMap::new()),
            blocked_prefixes: std::sync::RwLock::new(HashSet::new()),
            cache: std::sync::Mutex::new(HashMap::new()),
            cache_ttl: None,
            cache_enabled: false,
            failure_counts: std::sync::RwLock::new(HashMap::new()),
            privileged_tools: std::sync::RwLock::new(HashSet::new()),
            hooks: ToolHooks::new(),
            hooks_override: std::sync::Mutex::new(None),
            approval_queue: None,
            content_filter: None,
            audit_log: None,
            web_search_providers: None,
        }
    }

    /// Create a new registry with caching enabled
    pub fn with_cache(ttl: Duration) -> Self {
        Self {
            tools: std::sync::RwLock::new(HashMap::new()),
            dynamic_tools: std::sync::RwLock::new(HashMap::new()),
            blocked_prefixes: std::sync::RwLock::new(HashSet::new()),
            cache: std::sync::Mutex::new(HashMap::new()),
            cache_ttl: Some(ttl),
            cache_enabled: true,
            failure_counts: std::sync::RwLock::new(HashMap::new()),
            privileged_tools: std::sync::RwLock::new(HashSet::new()),
            hooks: ToolHooks::new(),
            hooks_override: std::sync::Mutex::new(None),
            approval_queue: None,
            content_filter: None,
            audit_log: None,
            web_search_providers: None,
        }
    }

    /// Attach the shared web_search provider list so hot-reload can update it
    /// without rebuilding the registry.
    pub fn with_web_search_providers(mut self, providers: WebSearchProviders) -> Self {
        self.web_search_providers = Some(providers);
        self
    }

    /// Get a clone of the shared web_search provider list, if one was set.
    pub fn web_search_providers(&self) -> Option<WebSearchProviders> {
        self.web_search_providers.clone()
    }

    // ── Circuit breaker ───────────────────────────────────────────────────────

    /// Record a failure for `name`. After `CIRCUIT_BREAKER_THRESHOLD`
    /// consecutive failures the tool is considered degraded and excluded from
    /// `get_available()`.
    pub fn record_failure(&self, name: &str) {
        if let Ok(mut counts) = self.failure_counts.write() {
            let entry = counts.entry(name.to_string()).or_insert(0);
            *entry += 1;
            if *entry >= Self::CIRCUIT_BREAKER_THRESHOLD {
                tracing::warn!(
                    tool = name,
                    failures = *entry,
                    "Tool circuit-breaker tripped — marking as degraded"
                );
            }
        }
    }

    /// Reset the failure count for `name` (e.g. after a successful execution).
    pub fn reset_failure(&self, name: &str) {
        if let Ok(mut counts) = self.failure_counts.write() {
            counts.remove(name);
        }
    }

    /// Returns `true` if the tool has been circuit-broken due to repeated
    /// failures.
    pub fn is_degraded(&self, name: &str) -> bool {
        self.failure_counts
            .read()
            .map(|counts| counts.get(name).copied().unwrap_or(0) >= Self::CIRCUIT_BREAKER_THRESHOLD)
            .unwrap_or(false)
    }

    /// List all currently-degraded tool names.
    pub fn degraded_tools(&self) -> Vec<String> {
        self.failure_counts
            .read()
            .map(|counts| {
                counts
                    .iter()
                    .filter(|(_, &v)| v >= Self::CIRCUIT_BREAKER_THRESHOLD)
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    // ── Privilege / trust-level filtering ────────────────────────────────────

    /// Mark `name` as a privileged tool (shell execution, file writes, etc.).
    /// Privileged tools are hidden when `context.skill_trust == Community`.
    pub fn mark_privileged(&mut self, name: &str) {
        if let Ok(mut set) = self.privileged_tools.write() {
            set.insert(name.to_string());
        }
    }

    /// Returns `true` if `name` is a privileged tool.
    pub fn is_privileged(&self, name: &str) -> bool {
        self.privileged_tools
            .read()
            .map(|set| set.contains(name))
            .unwrap_or(false)
    }

    /// Returns `true` if `name` matches any blocked prefix.
    fn is_blocked(&self, name: &str) -> bool {
        self.blocked_prefixes
            .read()
            .map(|set| set.iter().any(|p| name.starts_with(p.as_str())))
            .unwrap_or(false)
    }

    /// Returns `true` if the tool should be excluded from availability checks,
    /// considering blocked prefixes, circuit-breaker state, trust level,
    /// plugin allowlists, and any RBAC/gating policy attached to the context.
    fn is_excluded(&self, name: &str, context: &ToolContext) -> bool {
        if self.is_blocked(name) {
            return true;
        }
        if self.is_degraded(name) {
            return true;
        }
        if context.model.skill_trust < SkillTrust::Trusted && self.is_privileged(name) {
            return true;
        }

        // Determine registration provenance for source gating.
        let is_dynamic = self.is_dynamic_tool(name);
        let is_mcp = name.starts_with("mcp__");

        // Plugin allowlist at the context level (runtime restriction).
        if is_dynamic && Self::is_plugin_like_name(name) {
            if let Some(allowlist) = context.plugin_allowlist() {
                let allowed = allowlist
                    .iter()
                    .any(|prefix| name == prefix || name.starts_with(prefix));
                if !allowed {
                    return true;
                }
            }
        }

        // Sandbox policy: require sandboxed tools.
        if let Some(sandbox_policy) = context.sandbox_policy() {
            if sandbox_policy.require_sandboxed {
                let caps = self.tool_capabilities(name);
                if !caps.sandboxed {
                    return true;
                }
            }
        }

        if let (Some(user_ctx), Some(policy)) = (&context.user_context, &context.model.tool_policy)
        {
            let capabilities = self.tool_capabilities(name);
            let eval_ctx = PolicyEvaluationContext {
                model_name: context.model.model_name.clone(),
                provider_name: context.model.provider_name.clone(),
                sender_id: context.sender_id.clone(),
                sender_is_owner: context.sender_is_owner,
                plugin_allowlist: context.plugin_allowlist().map(|s| s.to_vec()),
                model_capabilities: context.model.model_capabilities.clone(),
                is_dynamic,
                is_mcp,
            };
            if !policy.evaluate_with_context(user_ctx, name, &capabilities, &eval_ctx) {
                return true;
            }
        }
        false
    }

    /// Helper to look up tool capabilities from either registry.
    fn tool_capabilities(&self, name: &str) -> crate::tools::sdk::ToolCapabilities {
        self.tools
            .read()
            .ok()
            .and_then(|map| map.get(name).map(|t| t.capabilities()))
            .or_else(|| {
                self.dynamic_tools
                    .read()
                    .ok()
                    .and_then(|map| map.get(name).map(|t| t.capabilities()))
            })
            .unwrap_or_default()
    }

    /// Get the advertised capabilities for a tool by name.
    pub fn get_capabilities(&self, name: &str) -> crate::tools::sdk::ToolCapabilities {
        self.tool_capabilities(name)
    }

    /// Returns `true` if `name` is registered only in the dynamic registry.
    fn is_dynamic_tool(&self, name: &str) -> bool {
        self.tools
            .read()
            .ok()
            .is_none_or(|map| !map.contains_key(name))
            && self
                .dynamic_tools
                .read()
                .map(|map| map.contains_key(name))
                .unwrap_or(false)
    }

    /// Heuristic: plugin tools often use `__` separators (MCP or plugin
    /// runtime).
    fn is_plugin_like_name(name: &str) -> bool {
        name.contains("__")
    }

    /// Enable caching with the specified TTL
    pub fn enable_cache(&mut self, ttl: Duration) {
        self.cache_enabled = true;
        self.cache_ttl = Some(ttl);
    }

    /// Disable caching
    pub fn disable_cache(&mut self) {
        self.cache_enabled = false;
        // Clear existing cache
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// Clear the tool result cache
    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    // ── Unified tool iteration ───────────────────────────────────────────────

    /// Iterate over both static and dynamic registries, yielding
    /// `(name, Arc<dyn Tool>)` for every tool that satisfies `filter`.
    ///
    /// This is the single point of iteration for `list()`, `get_definitions()`,
    /// `get_available()`, and `all_tools_arc()` — they all delegate here rather
    /// than duplicating the two-registry walk.
    fn iter_tools<F>(&self, filter: F) -> Vec<(String, Arc<dyn Tool>)>
    where
        F: Fn(&str) -> bool,
    {
        let mut result = Vec::new();
        if let Ok(map) = self.tools.read() {
            for (name, tool) in map.iter() {
                if filter(name) {
                    result.push((name.clone(), tool.clone()));
                }
            }
        }
        if let Ok(dynamic) = self.dynamic_tools.read() {
            for (name, tool) in dynamic.iter() {
                if filter(name) {
                    result.push((name.clone(), tool.clone()));
                }
            }
        }
        result
    }

    /// Generate a cache key from tool name and arguments
    fn cache_key(name: &str, args: &Value) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        // Hash the JSON string representation of args
        args.to_string().hash(&mut hasher);
        format!("{}:{}", name, hasher.finish())
    }

    /// Get cached result if available and not expired
    fn get_cached(&self, key: &str) -> Option<ToolExecutionResult> {
        if !self.cache_enabled {
            return None;
        }

        let cache = match self.cache.lock() {
            Ok(guard) => guard,
            Err(e) => {
                warn!("Cache mutex poisoned in get_cached: {}", e);
                return None;
            }
        };
        let entry = cache.get(key)?;

        // Check if cache entry is expired
        if let Some(ttl) = self.cache_ttl {
            if entry.timestamp.elapsed() > ttl {
                return None;
            }
        }

        Some(entry.result.clone())
    }

    /// Store result in cache
    fn store_cached(&self, key: String, result: ToolExecutionResult) {
        if !self.cache_enabled {
            return;
        }

        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(
                key,
                CacheEntry {
                    result,
                    timestamp: std::time::Instant::now(),
                },
            );
        }
    }

    // ── Hooks and approval queue ──────────────────────────────────────────────

    /// Set the hooks for this registry.
    ///
    /// Hooks allow policy decisions, before/after execution callbacks,
    /// and human-in-the-loop approval for high-risk tools.
    pub fn with_hooks(mut self, hooks: ToolHooks) -> Self {
        self.hooks = hooks;
        self
    }

    /// Set the hooks for this registry through `&self` (interior mutability).
    ///
    /// This allows setting hooks through an `Arc<ToolRegistry>` without
    /// requiring `&mut self`. Used by tests that need to inject policy
    /// hooks at runtime (e.g. auto-approval for device tool calls).
    pub fn set_hooks(&self, hooks: ToolHooks) {
        if let Ok(mut guard) = self.hooks_override.lock() {
            *guard = Some(hooks);
        }
    }

    /// Return the active hooks — the override hooks if set, otherwise the
    /// builder-configured hooks.  Override hooks take precedence so that
    /// `set_hooks()` (called through `Arc<ToolRegistry>`) can inject hooks
    /// at runtime without requiring `&mut self`.
    fn active_hooks(&self) -> ToolHooks {
        self.hooks_override
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| self.hooks.clone())
    }

    /// Set the approval queue for human-in-the-loop execution.
    ///
    /// When set, tool calls that return `ToolPolicyDecision::NeedsApproval`
    /// will suspend execution and wait for human approval via the queue.
    pub fn with_approval_queue(mut self, queue: Arc<ApprovalQueue>) -> Self {
        self.approval_queue = Some(queue);
        self
    }

    /// Get a reference to the approval queue if set.
    pub fn approval_queue(&self) -> Option<&Arc<ApprovalQueue>> {
        self.approval_queue.as_ref()
    }

    /// Set the content filter for scanning tool outputs.
    pub fn with_content_filter(
        mut self,
        filter: Arc<crate::security::content_filter::ContentFilter>,
    ) -> Self {
        self.content_filter = Some(filter);
        self
    }

    /// Set the audit logger for recording security events.
    pub fn with_audit_log(
        mut self,
        audit_log: Arc<dyn crate::security::runtime_audit::AuditLogger>,
    ) -> Self {
        self.audit_log = Some(audit_log);
        self
    }

    /// Get a clone of the configured audit logger, if any.
    pub fn audit_log(&self) -> Option<Arc<dyn crate::security::runtime_audit::AuditLogger>> {
        self.audit_log.clone()
    }

    /// Get a clone of the configured content filter, if any.
    pub fn content_filter(&self) -> Option<Arc<crate::security::content_filter::ContentFilter>> {
        self.content_filter.clone()
    }

    /// Return a snapshot of all dynamically-registered tools.
    pub fn dynamic_tools(&self) -> Vec<(String, std::sync::Arc<dyn Tool>)> {
        match self.dynamic_tools.read() {
            Ok(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Err(e) => {
                warn!("dynamic_tools lock poisoned: {}", e);
                Vec::new()
            }
        }
    }

    /// Register a tool from a boxed implementation.
    pub fn register(&mut self, tool: BoxedTool) {
        let name = tool.name().to_string();
        let tool: SharedTool = tool.into();
        match self.tools.write() {
            Ok(mut map) => {
                map.insert(name, tool);
            }
            Err(e) => warn!("Tools RwLock poisoned in register: {}", e),
        }
    }

    /// Remove a single tool by exact name.
    pub fn remove(&mut self, name: &str) -> Option<SharedTool> {
        match self.tools.write() {
            Ok(mut map) => map.remove(name),
            Err(e) => {
                warn!("Tools RwLock poisoned in remove: {}", e);
                None
            }
        }
    }

    /// Replace a statically-registered tool by exact name.
    /// Returns the previous tool if one existed.
    pub fn replace(&mut self, name: &str, tool: BoxedTool) -> Option<SharedTool> {
        let new_name = tool.name().to_string();
        if name != new_name {
            warn!(
                "Tool replacement name mismatch: replacing '{}' with '{}'",
                name, new_name
            );
        }
        let tool: SharedTool = tool.into();
        match self.tools.write() {
            Ok(mut map) => map.insert(new_name, tool),
            Err(e) => {
                warn!("Tools RwLock poisoned in replace: {}", e);
                None
            }
        }
    }

    /// Remove all tools whose names start with `prefix`.
    ///
    /// Uses interior mutability so it works through `Arc<ToolRegistry>` —
    /// tools are hidden from all lookup methods immediately. The underlying
    /// map entries are lazily cleaned up (they remain allocated but invisible).
    ///
    /// Used by the MCP subsystem to clean up `mcp__{server}__*` tools when a
    /// server disconnects.
    pub fn deregister_prefix(&self, prefix: &str) {
        if let Ok(mut set) = self.blocked_prefixes.write() {
            set.insert(prefix.to_string());
        }
        // Also remove matching static and dynamic tools immediately so
        // stale entries don't accumulate in memory.
        if let Ok(mut map) = self.tools.write() {
            map.retain(|k, _| !k.starts_with(prefix));
        }
        if let Ok(mut map) = self.dynamic_tools.write() {
            map.retain(|k, _| !k.starts_with(prefix));
        }
    }

    /// Dynamically register a tool without requiring `&mut self`.
    ///
    /// This allows tools to be added through an `Arc<ToolRegistry>` — used by
    /// the MCP subsystem to register auto-discovered tools at startup.
    pub fn register_dynamic(&self, tool: std::sync::Arc<dyn Tool>) {
        let name = tool.name().to_string();
        if let Ok(mut map) = self.dynamic_tools.write() {
            map.insert(name, tool);
        }
    }

    /// Remove a single dynamically-registered tool by exact name.
    pub fn deregister_dynamic(&self, name: &str) {
        if let Ok(mut map) = self.dynamic_tools.write() {
            map.remove(name);
        }
    }

    /// Get a tool by name (returns `None` for blocked or degraded tools).
    ///
    /// Only covers statically-registered tools. For dynamic tools use
    /// `execute()` or `execute_call()` which check both registries.
    pub fn get(&self, name: &str) -> Option<SharedTool> {
        if self.is_blocked(name) || self.is_degraded(name) {
            return None;
        }
        self.tools
            .read()
            .ok()
            .and_then(|map| map.get(name).cloned())
    }

    /// List available tool names (excludes blocked and degraded tools).
    /// Includes both statically- and dynamically-registered tools.
    /// List available tool names (excludes blocked and degraded tools).
    /// Includes both statically- and dynamically-registered tools.
    pub fn list(&self) -> Vec<String> {
        self.iter_tools(|name| !self.is_blocked(name) && !self.is_degraded(name))
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// Get all dynamically-registered tools as `Arc<dyn Tool>` references.
    ///
    /// Excludes blocked and degraded tools. Static tools registered via
    /// `register(Box<dyn Tool>)` are NOT returned — callers that need
    /// `Arc<dyn Tool>` for static tools should collect `Arc` references
    /// at registration time via `register_arc()`.
    pub fn all_tools_arc(&self) -> Vec<std::sync::Arc<dyn Tool>> {
        let mut result: Vec<std::sync::Arc<dyn Tool>> = Vec::new();

        if let Ok(dynamic) = self.dynamic_tools.read() {
            for (name, tool) in dynamic.iter() {
                if !self.is_blocked(name) && !self.is_degraded(name) {
                    result.push(tool.clone());
                }
            }
        }

        result
    }

    /// Check if a tool exists, is not blocked, and is not degraded.
    /// Checks both static and dynamic registries.
    pub fn has(&self, name: &str) -> bool {
        if self.is_blocked(name) || self.is_degraded(name) {
            return false;
        }
        if self
            .tools
            .read()
            .ok()
            .is_some_and(|map| map.contains_key(name))
        {
            return true;
        }
        self.dynamic_tools
            .read()
            .map(|map| map.contains_key(name))
            .unwrap_or(false)
    }

    /// Get all tools as function definitions (excludes blocked and degraded
    /// tools). Includes both statically- and dynamically-registered tools.
    pub fn get_definitions(&self) -> Vec<FunctionDefinition> {
        self.iter_tools(|name| !self.is_blocked(name) && !self.is_degraded(name))
            .into_iter()
            .map(|(_, tool)| tool.to_function_definition())
            .collect()
    }

    /// Get all available tools for a given context.
    ///
    /// Excludes:
    /// - Blocked-prefix tools (MCP server disconnected)
    /// - Degraded tools (circuit-breaker tripped)
    /// - Privileged tools when `context.skill_trust == Community`
    ///
    /// Includes both statically- and dynamically-registered tools.
    pub fn get_available(&self, context: &ToolContext) -> Vec<FunctionDefinition> {
        self.iter_tools(|name| !self.is_excluded(name, context))
            .into_iter()
            .filter(|(_, tool)| tool.is_available(context))
            .map(|(_, tool)| tool.to_function_definition())
            .collect()
    }

    /// Execute a tool by name with optional caching, hooks, and approval flow.
    /// Checks both static and dynamic registries.
    ///
    /// # Policy and Approval Flow
    ///
    /// Run policy hooks and the built-in `requires_approval` fallback.
    ///
    /// If no explicit policy hooks are configured but the tool advertises
    /// `requires_approval`, this synthesises a `NeedsApproval` decision
    /// automatically so that high-risk tools (device access, etc.) are
    /// never executed silently without the caller going through approval.
    async fn evaluate_policy(&self, name: &str, args: &Value) -> ToolPolicyDecision {
        let mut decision = self.active_hooks().run_policy(name, args).await;

        // requires_approval fallback — only when no policy hook exists, so
        // an explicitly-configured policy hook is always authoritative.
        if matches!(decision, ToolPolicyDecision::Allow) && !self.active_hooks().has_policy_hooks()
        {
            let caps = self.get_capabilities(name);
            if caps.requires_approval {
                let approval_id = format!(
                    "fallback-{}-{}",
                    name,
                    uuid::Uuid::new_v4()
                        .to_string()
                        .split('-')
                        .next()
                        .unwrap_or("0000")
                );
                decision = ToolPolicyDecision::NeedsApproval {
                    approval_id,
                    tool_name: name.to_string(),
                    args: args.clone(),
                    risk_level: crate::tools::approval::RiskLevel::High,
                    approval_level: crate::tools::approval::ApprovalLevel::Ask,
                    requested_by: "system".to_string(),
                    message: format!(
                        "Tool '{}' requires approval (fallsback from requires_approval flag)",
                        name
                    ),
                };
            }
        }

        decision
    }

    /// Execute a tool by name with optional caching, hooks, and approval flow.
    /// Checks both static and dynamic registries.
    ///
    /// # Policy and Approval Flow
    ///
    /// 1. Run policy hooks — if any hook returns `Deny`, return error
    ///    immediately
    /// 2. If any hook returns `NeedsApproval` and approval_queue is configured,
    ///    suspend execution and wait for human approval
    /// 3. Run before-hooks
    /// 4. Execute the tool
    /// 5. Run after-hooks
    pub async fn execute(
        &self,
        name: &str,
        args: Value,
        context: &ToolContext,
    ) -> Option<crate::Result<ToolExecutionResult>> {
        let policy_decision = self.evaluate_policy(name, &args).await;

        match policy_decision {
            ToolPolicyDecision::Allow => {
                // Proceed with execution
            }
            ToolPolicyDecision::Deny { reason } => {
                return Some(Err(crate::error::SyscityError::Validation(format!(
                    "Tool '{}' denied: {}",
                    name, reason
                ))));
            }
            ToolPolicyDecision::NeedsApproval {
                approval_id,
                tool_name,
                args: approval_args,
                risk_level,
                approval_level,
                requested_by,
                message,
            } => {
                // Check if approval queue is configured
                let approval_queue = match &self.approval_queue {
                    Some(q) => q.clone(),
                    None => {
                        return Some(Err(crate::error::SyscityError::Validation(
                            "Tool requires approval but no approval queue configured".into(),
                        )));
                    }
                };

                // Create oneshot channel for the approval resolution
                let (tx, rx) = tokio::sync::oneshot::channel();

                // Create pending approval
                let approval = PendingApproval::new(
                    &approval_id,
                    &tool_name,
                    approval_args,
                    requested_by,
                    risk_level,
                    approval_level,
                    message,
                    tx,
                );

                // Submit to approval queue
                approval_queue.submit(approval).await;

                // Wait for human decision (with 5-minute timeout)
                const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
                match tokio::time::timeout(APPROVAL_TIMEOUT, rx).await {
                    Ok(Ok(ApprovalDecision::Approve)) => {
                        tracing::info!(
                            "Approval {} granted, proceeding with tool execution",
                            approval_id
                        );
                        // Proceed with execution below
                    }
                    Ok(Ok(ApprovalDecision::Deny { reason })) => {
                        return Some(Err(crate::error::SyscityError::Validation(format!(
                            "Tool '{}' denied by user: {}",
                            name, reason
                        ))));
                    }
                    Ok(Err(_)) => {
                        return Some(Err(crate::error::SyscityError::Validation(
                            "Approval channel closed".into(),
                        )));
                    }
                    Err(_) => {
                        return Some(Err(crate::error::SyscityError::Timeout(format!(
                            "Tool '{}' approval request timed out after {:?}",
                            name, APPROVAL_TIMEOUT
                        ))));
                    }
                }
            }
        }

        // Run before-hooks
        self.active_hooks().run_before(name, &args).await;

        // Check cache first
        let cache_key = Self::cache_key(name, &args);
        if let Some(cached_result) = self.get_cached(&cache_key) {
            tracing::debug!("Cache hit for tool: {}", name);
            let result = Ok(cached_result);
            if let Ok(ref exec_result) = result {
                self.active_hooks()
                    .run_after(name, &args, exec_result)
                    .await;
            }
            return self.filter_and_audit(name, context, Some(result)).await;
        }

        // Execute the tool — clone args so the original remains for after-hooks
        let execution_result: Option<crate::Result<ToolExecutionResult>> = {
            // Try static tools first
            if let Some(tool) = self.get(name) {
                let result = tool.execute(args.clone(), context).await;
                if let Ok(ref exec_result) = result {
                    self.store_cached(cache_key, exec_result.clone());
                }
                Some(result)
            } else {
                // Try dynamic tools
                let dynamic_tool = self
                    .dynamic_tools
                    .read()
                    .ok()
                    .and_then(|map| map.get(name).cloned());

                if let Some(tool) = dynamic_tool {
                    if !self.is_blocked(name) && !self.is_degraded(name) {
                        let result = tool.execute(args.clone(), context).await;
                        if let Ok(ref exec_result) = result {
                            self.store_cached(cache_key, exec_result.clone());
                        }
                        Some(result)
                    } else {
                        Some(Err(crate::error::SyscityError::Validation(format!(
                            "Tool '{}' is blocked or degraded",
                            name
                        ))))
                    }
                } else {
                    None
                }
            }
        };

        // Run after-hooks
        if let Some(Ok(ref exec_result)) = execution_result {
            self.active_hooks()
                .run_after(name, &args, exec_result)
                .await;
        }

        self.filter_and_audit(name, context, execution_result).await
    }

    /// Apply content filtering and audit logging to a tool execution result.
    async fn filter_and_audit(
        &self,
        name: &str,
        context: &ToolContext,
        result: Option<crate::Result<ToolExecutionResult>>,
    ) -> Option<crate::Result<ToolExecutionResult>> {
        // ── Audit: tool invocation ─────────────────────────────────────────
        if let Some(ref audit) = self.audit_log {
            let allowed = matches!(result, Some(Ok(_)));
            audit
                .log_entry(
                    crate::security::runtime_audit::AuditEventType::ToolInvocation,
                    context.user_id.clone(),
                    name.to_string(),
                    allowed,
                    format!("Tool '{}' executed", name),
                    None,
                )
                .await;
        }

        // ── Content filtering ──────────────────────────────────────────────
        let result = match result {
            Some(Ok(exec_result)) => {
                if let Some(ref filter) = self.content_filter {
                    let outcome = filter.filter_result(&exec_result);

                    // Audit: content filter action
                    if let Some(ref audit) = self.audit_log {
                        if outcome.action != crate::security::content_filter::FilterAction::Pass {
                            let details = serde_json::json!({
                                "action": format!("{:?}", outcome.action),
                                "pii_findings": outcome.pii_findings.len(),
                                "secret_findings": outcome.secret_findings.len(),
                                "summary": outcome.summary,
                            });
                            audit
                                .log_entry(
                                    crate::security::runtime_audit::AuditEventType::ContentFilter,
                                    context.user_id.clone(),
                                    name.to_string(),
                                    outcome.action
                                        != crate::security::content_filter::FilterAction::Blocked,
                                    outcome.summary.clone(),
                                    Some(details),
                                )
                                .await;
                        }
                    }

                    let filtered = crate::tools::ToolExecutionResult {
                        success: if outcome.action
                            == crate::security::content_filter::FilterAction::Blocked
                        {
                            false
                        } else {
                            outcome.success
                        },
                        output: outcome.output,
                        error: if outcome.action
                            == crate::security::content_filter::FilterAction::Blocked
                        {
                            Some(outcome.summary)
                        } else {
                            exec_result.error
                        },
                        data: outcome.data,
                        execution_time: exec_result.execution_time,
                    };
                    Some(Ok(filtered))
                } else {
                    Some(Ok(exec_result))
                }
            }
            other => other,
        };

        result
    }

    /// Execute a tool by name, skipping the cache layer but still running the
    /// full policy, approval, hooks, and audit pipeline.
    ///
    /// Returns `None` only when the tool name is unknown (not registered).
    /// Blocked, degraded, or policy-denied tools return `Some(Err(...))`
    /// so callers can distinguish "not found" from "rejected".
    ///
    /// This is `pub(crate)` for use by other modules in the crate
    /// (e.g. streaming execution paths) that need to bypass the cache
    /// without sacrificing safety checks, though currently no external
    /// caller exists.
    #[cfg(test)]
    pub(crate) async fn execute_no_cache(
        &self,
        name: &str,
        args: Value,
        context: &ToolContext,
    ) -> Option<crate::Result<ToolExecutionResult>> {
        // Run policy evaluation (approval, denials, hooks all handled here).
        let policy_decision = self.evaluate_policy(name, &args).await;
        match policy_decision {
            ToolPolicyDecision::Allow => { /* proceed */ }
            ToolPolicyDecision::Deny { reason } => {
                return Some(Err(crate::error::SyscityError::Validation(format!(
                    "Tool '{}' denied: {}",
                    name, reason
                ))));
            }
            ToolPolicyDecision::NeedsApproval { .. } => {
                // execute_no_cache does not support the full approval flow;
                // callers should use `execute()` instead.
                return Some(Err(crate::error::SyscityError::Validation(format!(
                    "Tool '{}' requires approval; use execute() instead of execute_no_cache",
                    name,
                ))));
            }
        }

        // Run before-hooks
        self.active_hooks().run_before(name, &args).await;

        // Execute the tool
        let execution_result: Option<crate::Result<ToolExecutionResult>> = {
            // Try static tools first
            if let Some(tool) = self.get(name) {
                Some(tool.execute(args.clone(), context).await)
            } else {
                // Try dynamic tools
                let dynamic_tool = self
                    .dynamic_tools
                    .read()
                    .ok()
                    .and_then(|map| map.get(name).cloned());
                if let Some(tool) = dynamic_tool {
                    if !self.is_blocked(name) && !self.is_degraded(name) {
                        Some(tool.execute(args.clone(), context).await)
                    } else {
                        Some(Err(crate::error::SyscityError::Validation(format!(
                            "Tool '{}' is blocked or degraded",
                            name
                        ))))
                    }
                } else {
                    None
                }
            }
        };

        // Run after-hooks
        if let Some(Ok(ref exec_result)) = execution_result {
            self.active_hooks()
                .run_after(name, &args, exec_result)
                .await;
        }

        self.filter_and_audit(name, context, execution_result).await
    }

    /// Execute a function call from an LLM.
    /// Checks both static and dynamic registries.
    /// Enforces the timeout configured in `ToolContext`.
    pub async fn execute_call(
        &self,
        call: &FunctionCall,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let args: Value = if call.arguments.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&call.arguments).map_err(|e| {
                crate::error::SyscityError::Validation(format!(
                    "Invalid arguments for tool {}: {}",
                    call.name, e
                ))
            })?
        };

        let tool_name = call.name.clone();
        let timeout = context.timeout();

        // Try static tools first
        if let Some(tool) = self.get(&tool_name) {
            return tokio::time::timeout(timeout, tool.execute(args, context))
                .await
                .map_err(|_| {
                    crate::error::SyscityError::Timeout(format!(
                        "Tool '{}' timed out after {:?}",
                        tool_name, timeout
                    ))
                })?;
        }

        // Try dynamic tools
        let dynamic_tool = self
            .dynamic_tools
            .read()
            .ok()
            .and_then(|map| map.get(&tool_name).cloned());

        if let Some(tool) = dynamic_tool {
            if !self.is_blocked(&tool_name) && !self.is_degraded(&tool_name) {
                return tokio::time::timeout(timeout, tool.execute(args, context))
                    .await
                    .map_err(|_| {
                        crate::error::SyscityError::Timeout(format!(
                            "Tool '{}' timed out after {:?}",
                            tool_name, timeout
                        ))
                    })?;
            }
        }

        Err(crate::error::SyscityError::Validation(format!(
            "Unknown tool: {}. Available tools: {}",
            tool_name,
            self.list().join(", ")
        )))
    }

    /// Execute a function call from an LLM with streaming output.
    ///
    /// Policy hooks, approval, and before-hooks are run before chunks are
    /// yielded. `on_chunk` is invoked for every [`ToolExecutionChunk`]
    /// produced by the tool. After the stream completes, after-hooks,
    /// content filtering, and audit logging are applied and the final
    /// [`ToolExecutionResult`] is returned.
    ///
    /// This method owns the tool reference internally, so it works for both
    /// static and dynamically-registered tools without lifetime issues.
    pub async fn execute_call_streaming<F, Fut>(
        &self,
        call: &FunctionCall,
        context: &ToolContext,
        mut on_chunk: F,
    ) -> crate::Result<ToolExecutionResult>
    where
        F: FnMut(ToolExecutionChunk) -> Fut + Send,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let args: Value = if call.arguments.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&call.arguments).map_err(|e| {
                crate::error::SyscityError::Validation(format!(
                    "Invalid arguments for tool {}: {}",
                    call.name, e
                ))
            })?
        };

        let tool_name = call.name.clone();

        let policy_decision = self.evaluate_policy(&tool_name, &args).await;
        match policy_decision {
            // Allow → proceed to execution below.
            ToolPolicyDecision::Allow => {}
            ToolPolicyDecision::Deny { reason } => {
                return Err(crate::error::SyscityError::Validation(format!(
                    "Tool '{}' denied: {}",
                    tool_name, reason
                )));
            }
            ToolPolicyDecision::NeedsApproval { .. } => {
                // For streaming tools, fall back to buffered execution so the
                // approval flow can suspend and resume in a single future.
                let result = self.execute(&tool_name, args.clone(), context).await;
                return match result {
                    Some(Ok(exec_result)) => {
                        if !exec_result.output.is_empty() {
                            on_chunk(ToolExecutionChunk::Output(exec_result.output.clone())).await;
                        }
                        if let Some(error) = exec_result.error.clone() {
                            on_chunk(ToolExecutionChunk::Error(error)).await;
                        }
                        if let Some(data) = exec_result.data.clone() {
                            on_chunk(ToolExecutionChunk::Data(data)).await;
                        }
                        Ok(exec_result)
                    }
                    Some(Err(e)) => {
                        on_chunk(ToolExecutionChunk::Error(e.to_string())).await;
                        Err(e)
                    }
                    None => Err(crate::error::SyscityError::Validation(format!(
                        "Tool '{}' was found but could not be executed (may have been deregistered)",
                        tool_name,
                    ))),
                };
            }
        }

        // Run before-hooks.
        self.active_hooks().run_before(&tool_name, &args).await;

        // Look up the tool and consume its stream.
        let collected = if let Some(tool) = self.get(&tool_name) {
            consume_stream(tool.execute_stream(args.clone(), context), &mut on_chunk).await
        } else {
            let dynamic_tool = self
                .dynamic_tools
                .read()
                .ok()
                .and_then(|map| map.get(&tool_name).cloned());
            if let Some(tool) = dynamic_tool {
                if !self.is_blocked(&tool_name) && !self.is_degraded(&tool_name) {
                    consume_stream(tool.execute_stream(args.clone(), context), &mut on_chunk).await
                } else {
                    return Err(crate::error::SyscityError::Validation(format!(
                        "Tool '{}' is blocked or degraded",
                        tool_name
                    )));
                }
            } else {
                return Err(crate::error::SyscityError::Validation(format!(
                    "Unknown tool: {}. Available tools: {}",
                    tool_name,
                    self.list().join(", ")
                )));
            }
        };

        // Apply after-hooks, content filtering, and audit logging.
        match self
            .finalize_stream_result(&tool_name, &args, context, collected)
            .await
        {
            Some(Ok(result)) => Ok(result),
            Some(Err(e)) => Err(e),
            None => Err(crate::error::SyscityError::Validation(format!(
                "Tool '{}' finalization failed",
                tool_name
            ))),
        }
    }

    /// Apply content filtering and audit logging to a collected streaming
    /// result, and run after-hooks.
    ///
    /// This is the streaming equivalent of the post-processing performed by
    /// [`execute`](ToolRegistry::execute) after a buffered call.
    pub async fn finalize_stream_result(
        &self,
        name: &str,
        args: &Value,
        context: &ToolContext,
        collected: ToolExecutionResult,
    ) -> Option<crate::Result<ToolExecutionResult>> {
        self.active_hooks().run_after(name, args, &collected).await;
        self.filter_and_audit(name, context, Some(Ok(collected)))
            .await
    }
}

/// Consume a tool execution stream, accumulating chunks into a
/// [`ToolExecutionResult`] while invoking `on_chunk` for each chunk.
async fn consume_stream<S, F, Fut>(mut stream: S, on_chunk: &mut F) -> ToolExecutionResult
where
    S: tokio_stream::Stream<Item = ToolExecutionChunk> + Unpin,
    F: FnMut(ToolExecutionChunk) -> Fut + Send,
    Fut: std::future::Future<Output = ()> + Send,
{
    let mut output = String::new();
    let mut error_output = String::new();
    let mut data: Option<Value> = None;

    while let Some(chunk) = stream.next().await {
        match chunk {
            ToolExecutionChunk::Output(text) => {
                output.push_str(&text);
                on_chunk(ToolExecutionChunk::Output(text)).await;
            }
            ToolExecutionChunk::Error(text) => {
                error_output.push_str(&text);
                on_chunk(ToolExecutionChunk::Error(text)).await;
            }
            ToolExecutionChunk::Data(value) => {
                let value_clone = value.clone();
                data = Some(value);
                on_chunk(ToolExecutionChunk::Data(value_clone)).await;
            }
            ToolExecutionChunk::Done => {
                on_chunk(ToolExecutionChunk::Done).await;
            }
        }
    }

    let success = error_output.is_empty();
    let final_output = if error_output.is_empty() {
        output
    } else if output.is_empty() {
        error_output.clone()
    } else {
        format!("{}\nErrors:\n{}", output, error_output)
    };

    ToolExecutionResult {
        success,
        output: final_output,
        error: if success { None } else { Some(error_output) },
        data,
        execution_time: Duration::default(),
    }
}

/// ToolRegistrar for dynamic tool registration with validation
#[derive(Debug, Default)]
pub struct ToolRegistrar {
    registry: ToolRegistry,
    validators: Vec<Box<dyn ToolValidator>>,
}

/// Trait for custom tool validators
pub trait ToolValidator: Send + Sync + std::fmt::Debug {
    /// Validate a tool before registration
    fn validate(&self, tool: &dyn Tool) -> Result<(), ToolValidationError>;
    /// Validate tool input arguments
    fn validate_input(&self, tool_name: &str, args: &Value) -> Result<(), ToolValidationError>;
}

/// Tool validation errors
#[derive(Debug, Clone)]
pub enum ToolValidationError {
    /// Invalid tool name
    InvalidName(String),
    /// Invalid schema
    InvalidSchema(String),
    /// Input validation failed
    InvalidInput(String),
    /// Security violation
    SecurityViolation(String),
}

impl std::fmt::Display for ToolValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName(s) => write!(f, "Invalid tool name: {}", s),
            Self::InvalidSchema(s) => write!(f, "Invalid tool schema: {}", s),
            Self::InvalidInput(s) => write!(f, "Invalid tool input: {}", s),
            Self::SecurityViolation(s) => write!(f, "Security violation: {}", s),
        }
    }
}

impl std::error::Error for ToolValidationError {}

/// Name validator - ensures tool names follow conventions
#[derive(Debug)]
pub struct NameValidator;

impl ToolValidator for NameValidator {
    fn validate(&self, tool: &dyn Tool) -> Result<(), ToolValidationError> {
        let name = tool.name();

        // Check length
        if name.len() < 2 || name.len() > 64 {
            return Err(ToolValidationError::InvalidName(format!(
                "Tool name '{}' must be between 2 and 64 characters",
                name
            )));
        }

        // Check characters (alphanumeric, underscore, hyphen only)
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(ToolValidationError::InvalidName(format!(
                "Tool name '{}' contains invalid characters. Use alphanumeric, underscore, or \
                 hyphen only",
                name
            )));
        }

        // Check doesn't start with number
        if name.chars().next().map(|c| c.is_numeric()).unwrap_or(false) {
            return Err(ToolValidationError::InvalidName(format!(
                "Tool name '{}' cannot start with a number",
                name
            )));
        }

        Ok(())
    }

    fn validate_input(&self, _tool_name: &str, _args: &Value) -> Result<(), ToolValidationError> {
        Ok(())
    }
}

/// Schema validator - validates JSON schemas
#[derive(Debug)]
pub struct SchemaValidator;

impl ToolValidator for SchemaValidator {
    fn validate(&self, tool: &dyn Tool) -> Result<(), ToolValidationError> {
        let schema = tool.parameters_schema();

        // Check schema has required fields
        if !schema.get("type").map(|v| v == "object").unwrap_or(false) {
            return Err(ToolValidationError::InvalidSchema(
                "Schema must have type 'object'".to_string(),
            ));
        }

        if schema.get("properties").is_none() {
            return Err(ToolValidationError::InvalidSchema(
                "Schema must have 'properties' field".to_string(),
            ));
        }

        Ok(())
    }

    fn validate_input(&self, tool_name: &str, args: &Value) -> Result<(), ToolValidationError> {
        // Basic JSON structure validation
        if !args.is_object() && !args.is_null() {
            return Err(ToolValidationError::InvalidInput(format!(
                "Tool '{}' arguments must be a JSON object",
                tool_name
            )));
        }

        Ok(())
    }
}

/// Security validator - checks for dangerous patterns
#[derive(Debug)]
pub struct SecurityValidator;

impl SecurityValidator {
    /// Check for path traversal attempts
    fn check_path_traversal(&self, value: &str) -> Result<(), ToolValidationError> {
        let dangerous_patterns = ["../", "..\\", "~/..", "/..", "%2e%2e%2f", "%252e%252e%252f"];

        for pattern in &dangerous_patterns {
            if value.contains(pattern) {
                return Err(ToolValidationError::SecurityViolation(format!(
                    "Path traversal attempt detected: {}",
                    pattern
                )));
            }
        }

        // Check for double slashes (can be used in some path traversal attacks)
        if value.contains("//") || value.contains("\\\\") {
            return Err(ToolValidationError::SecurityViolation(
                "Suspicious path pattern detected".to_string(),
            ));
        }

        Ok(())
    }

    /// Check for command injection attempts
    fn check_command_injection(&self, value: &str) -> Result<(), ToolValidationError> {
        let dangerous_chars = [';', '&', '|', '$', '`', '\n', '\r'];

        for ch in &dangerous_chars {
            if value.contains(*ch) {
                return Err(ToolValidationError::SecurityViolation(format!(
                    "Command injection attempt detected: contains '{}'",
                    ch
                )));
            }
        }

        // Check for command substitution patterns
        if value.contains("$(") || value.contains("${") {
            return Err(ToolValidationError::SecurityViolation(
                "Command substitution pattern detected".to_string(),
            ));
        }

        Ok(())
    }
}

impl ToolValidator for SecurityValidator {
    fn validate(&self, tool: &dyn Tool) -> Result<(), ToolValidationError> {
        // Check tool description for potential issues
        let desc = tool.description();
        if desc.len() < 10 {
            return Err(ToolValidationError::InvalidSchema(
                "Tool description must be at least 10 characters".to_string(),
            ));
        }

        Ok(())
    }

    fn validate_input(&self, _tool_name: &str, args: &Value) -> Result<(), ToolValidationError> {
        // Recursively check all string values for security issues
        fn check_value(
            value: &Value,
            validator: &SecurityValidator,
        ) -> Result<(), ToolValidationError> {
            match value {
                Value::String(s) => {
                    validator.check_path_traversal(s)?;
                    validator.check_command_injection(s)?;
                    Ok(())
                }
                Value::Array(arr) => {
                    for item in arr {
                        check_value(item, validator)?;
                    }
                    Ok(())
                }
                Value::Object(obj) => {
                    for (k, v) in obj {
                        // Also check keys for path traversal in property names
                        validator.check_path_traversal(k)?;
                        check_value(v, validator)?;
                    }
                    Ok(())
                }
                _ => Ok(()),
            }
        }

        check_value(args, self)
    }
}

impl ToolRegistrar {
    /// Create a new ToolRegistrar with default validators
    pub fn new() -> Self {
        Self {
            registry: ToolRegistry::new(),
            validators: vec![
                Box::new(NameValidator),
                Box::new(SchemaValidator),
                Box::new(SecurityValidator),
            ],
        }
    }

    /// Create with custom validators
    pub fn with_validators(validators: Vec<Box<dyn ToolValidator>>) -> Self {
        Self {
            registry: ToolRegistry::new(),
            validators,
        }
    }

    /// Register a tool with validation
    pub fn register(&mut self, tool: BoxedTool) -> Result<(), ToolValidationError> {
        // Run all validators
        for validator in &self.validators {
            validator.validate(tool.as_ref())?;
        }

        self.registry.register(tool);
        Ok(())
    }

    /// Validate tool input before execution
    pub fn validate_input(&self, tool_name: &str, args: &Value) -> Result<(), ToolValidationError> {
        for validator in &self.validators {
            validator.validate_input(tool_name, args)?;
        }
        Ok(())
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<SharedTool> {
        self.registry.get(name)
    }

    /// List available tool names
    pub fn list(&self) -> Vec<String> {
        self.registry.list()
    }

    /// Check if a tool exists
    pub fn has(&self, name: &str) -> bool {
        self.registry.has(name)
    }

    /// Get tool descriptions
    pub fn get_descriptions(&self) -> HashMap<String, String> {
        self.registry
            .list()
            .into_iter()
            .filter_map(|name| {
                self.registry
                    .get(&name)
                    .map(|t| (name.clone(), t.description().to_string()))
            })
            .collect()
    }

    /// Execute a tool with validation
    pub async fn execute(
        &self,
        name: &str,
        args: Value,
        context: &ToolContext,
    ) -> Option<crate::Result<ToolExecutionResult>> {
        // Validate input first
        if let Err(e) = self.validate_input(name, &args) {
            return Some(Err(crate::error::SyscityError::Validation(e.to_string())));
        }

        self.registry.execute(name, args, context).await
    }

    /// Get all tools as function definitions
    pub fn get_definitions(&self) -> Vec<FunctionDefinition> {
        self.registry.get_definitions()
    }

    /// Add a custom validator
    pub fn add_validator(&mut self, validator: Box<dyn ToolValidator>) {
        self.validators.push(validator);
    }

    /// Get reference to inner registry
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }
}

/// Helper function to create a JSON schema for a tool
pub fn create_schema(
    description: impl Into<String>,
    properties: Value,
    required: Vec<impl Into<String>>,
) -> Value {
    let required: Vec<String> = required.into_iter().map(Into::into).collect();

    serde_json::json!({
        "type": "object",
        "description": description.into(),
        "properties": properties,
        "required": required,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_id() {
        let id = ToolId::new("test_tool");
        assert_eq!(id.0, "test_tool");
    }

    #[test]
    fn test_tool_context() {
        let ctx = ToolContext::new("user1", "conv1")
            .with_timeout(Duration::from_secs(60))
            .allow_path("/tmp")
            .allow_command("ls");

        assert_eq!(ctx.user_id, "user1");
        assert_eq!(ctx.timeout(), Duration::from_secs(60));
        assert!(ctx.is_command_allowed("ls"));
        assert!(!ctx.is_command_allowed("rm"));
    }

    #[test]
    fn test_tool_execution_result() {
        let success = ToolExecutionResult::success("Done!");
        assert!(success.success);
        assert_eq!(success.output, "Done!");

        let error = ToolExecutionResult::error("Failed!");
        assert!(!error.success);
        assert_eq!(error.error, Some("Failed!".to_string()));
    }

    #[test]
    fn test_tool_registry() {
        let registry = ToolRegistry::new();
        assert!(registry.list().is_empty());
        assert!(!registry.has("test"));
    }

    #[test]
    fn test_create_schema() {
        let schema = create_schema(
            "A test tool",
            serde_json::json!({
                "name": { "type": "string" },
                "count": { "type": "integer" }
            }),
            vec!["name"],
        );

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["description"], "A test tool");
        assert_eq!(schema["required"], serde_json::json!(["name"]));
    }

    // ToolRegistrar tests

    #[test]
    fn test_tool_registrar_creation() {
        let registrar = ToolRegistrar::new();
        assert!(registrar.list().is_empty());
    }

    #[test]
    fn test_name_validator_valid() {
        struct ValidTool;

        #[async_trait]
        impl Tool for ValidTool {
            fn name(&self) -> &str {
                "valid_tool"
            }
            fn description(&self) -> &str {
                "A valid test tool"
            }
            fn parameters_schema(&self) -> Value {
                create_schema("Test", serde_json::json!({}), Vec::<String>::new())
            }
            async fn execute(
                &self,
                _args: Value,
                _ctx: &ToolContext,
            ) -> crate::Result<ToolExecutionResult> {
                Ok(ToolExecutionResult::success("ok"))
            }
        }

        let validator = NameValidator;
        assert!(validator.validate(&ValidTool).is_ok());
    }

    #[test]
    fn test_name_validator_invalid() {
        struct InvalidTool;

        #[async_trait]
        impl Tool for InvalidTool {
            fn name(&self) -> &str {
                "123_invalid"
            }
            fn description(&self) -> &str {
                "A test tool"
            }
            fn parameters_schema(&self) -> Value {
                create_schema("Test", serde_json::json!({}), Vec::<String>::new())
            }
            async fn execute(
                &self,
                _args: Value,
                _ctx: &ToolContext,
            ) -> crate::Result<ToolExecutionResult> {
                Ok(ToolExecutionResult::success("ok"))
            }
        }

        let validator = NameValidator;
        let result = validator.validate(&InvalidTool);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ToolValidationError::InvalidName(_)));
    }

    #[test]
    fn test_security_validator_path_traversal() {
        let validator = SecurityValidator;

        // Valid paths
        assert!(validator
            .check_path_traversal("/home/user/file.txt")
            .is_ok());
        assert!(validator.check_path_traversal("./file.txt").is_ok());

        // Invalid paths with traversal
        assert!(validator.check_path_traversal("../etc/passwd").is_err());
        assert!(validator
            .check_path_traversal("foo/../../../etc/passwd")
            .is_err());
    }

    #[test]
    fn test_security_validator_command_injection() {
        let validator = SecurityValidator;

        // Valid commands
        assert!(validator.check_command_injection("ls -la").is_ok());
        assert!(validator.check_command_injection("cat file.txt").is_ok());

        // Invalid commands with injection
        assert!(validator.check_command_injection("ls; rm -rf /").is_err());
        assert!(validator
            .check_command_injection("cat file | grep test")
            .is_err());
        assert!(validator.check_command_injection("echo $(whoami)").is_err());
    }

    #[test]
    fn test_security_validator_input_validation() {
        let validator = SecurityValidator;

        // Valid input
        let valid_args = serde_json::json!({
            "path": "/home/user/file.txt",
            "content": "hello world"
        });
        assert!(validator.validate_input("test", &valid_args).is_ok());

        // Invalid input with path traversal
        let invalid_args = serde_json::json!({
            "path": "../../../etc/passwd",
            "content": "malicious"
        });
        assert!(validator.validate_input("test", &invalid_args).is_err());

        // Invalid input with command injection
        let cmd_inject_args = serde_json::json!({
            "command": "ls; rm -rf /"
        });
        assert!(validator.validate_input("test", &cmd_inject_args).is_err());
    }

    // ── Path sandbox penetration tests ───────────────────────────────────────

    use tempfile::tempdir;

    fn workspace_context(root: &std::path::Path) -> ToolContext {
        ToolContext::new("user", "conv1")
            .with_workspace_root(root)
            .with_workspace_only(true)
    }

    #[test]
    fn test_path_allowed_relative_within_workspace() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        let ctx = workspace_context(&ws);
        assert!(ctx.is_path_allowed(std::path::Path::new("notes.txt")));
    }

    #[test]
    fn test_path_allowed_relative_traversal_outside_workspace() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let ctx = workspace_context(&ws);
        assert!(!ctx.is_path_allowed(std::path::Path::new("../outside.txt")));
    }

    #[test]
    fn test_path_allowed_dotdot_combo_traversal() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir_all(ws.join("subdir")).unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let ctx = workspace_context(&ws);
        assert!(!ctx.is_path_allowed(std::path::Path::new("subdir/../../outside.txt")));
    }

    #[test]
    fn test_path_allowed_absolute_outside_workspace() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        let ctx = workspace_context(&ws);
        assert!(!ctx.is_path_allowed(std::path::Path::new("/etc/passwd")));
    }

    #[test]
    fn test_path_allowed_absolute_inside_workspace() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        let inside = ws.join("file.txt");
        std::fs::write(&inside, "ok").unwrap();
        let ctx = workspace_context(&ws);
        assert!(ctx.is_path_allowed(&inside));
    }

    #[cfg(unix)]
    #[test]
    fn test_path_allowed_symlink_escape() {
        use std::os::unix::fs::symlink;
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, "secret").unwrap();
        symlink(&outside, ws.join("link")).unwrap();
        let ctx = workspace_context(&ws);
        assert!(!ctx.is_path_allowed(std::path::Path::new("link/secret.txt")));
    }

    #[test]
    fn test_path_allowed_tilde_expansion_outside_workspace() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        let ctx = workspace_context(&ws);
        // `~` resolves to the user's home directory, which is outside the workspace.
        assert!(!ctx.is_path_allowed(std::path::Path::new("~/anything.txt")));
    }

    #[test]
    fn test_path_allowed_with_allowlist() {
        let tmp = tempdir().unwrap();
        let allowed = tmp.path().join("allowed");
        std::fs::create_dir(&allowed).unwrap();
        let ctx = ToolContext::new("user", "conv1")
            .with_workspace_only(false)
            .allow_path(&allowed);
        let inside = allowed.join("file.txt");
        assert!(ctx.is_path_allowed(&inside));
        assert!(!ctx.is_path_allowed(std::path::Path::new("/etc/passwd")));
    }

    // ── Skill trust boundary tests ───────────────────────────────────────────

    struct ReadTool;

    #[async_trait]
    impl Tool for ReadTool {
        fn name(&self) -> &str {
            "read"
        }

        fn description(&self) -> &str {
            "Read a file"
        }

        fn parameters_schema(&self) -> Value {
            create_schema(
                "Read a file",
                serde_json::json!({"path": {"type": "string"}}),
                vec!["path"],
            )
        }

        async fn execute(
            &self,
            _args: Value,
            _ctx: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            Ok(ToolExecutionResult::success("ok"))
        }
    }

    #[test]
    fn test_skill_trust_boundary_excludes_privileged_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ShellTool::new()));
        registry.register(Box::new(ReadTool));
        registry.mark_privileged("shell");

        let trusted_ctx = ToolContext::new("user", "conv1").with_skill_trust(SkillTrust::Trusted);
        let community_ctx =
            ToolContext::new("user", "conv1").with_skill_trust(SkillTrust::Community);

        let trusted: Vec<String> = registry
            .get_available(&trusted_ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();
        let community: Vec<String> = registry
            .get_available(&community_ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert!(trusted.contains(&"shell".to_string()), "Trusted context should see shell");
        assert!(trusted.contains(&"read".to_string()), "Trusted context should see read");
        assert!(
            !community.contains(&"shell".to_string()),
            "Community context must not see shell"
        );
        assert!(community.contains(&"read".to_string()), "Community context should see read");
    }

    // ── RBAC policy tests ────────────────────────────────────────────────────

    struct HighRiskTool;

    #[async_trait]
    impl Tool for HighRiskTool {
        fn name(&self) -> &str {
            "high_risk"
        }

        fn description(&self) -> &str {
            "A high-risk tool"
        }

        fn parameters_schema(&self) -> Value {
            create_schema(
                "A high-risk tool",
                serde_json::json!({"x": {"type": "string"}}),
                vec!["x"],
            )
        }

        fn capabilities(&self) -> crate::tools::sdk::ToolCapabilities {
            crate::tools::sdk::ToolCapabilities {
                risk_level: crate::tools::approval::RiskLevel::High,
                categories: vec!["system".to_string()],
                ..Default::default()
            }
        }

        async fn execute(
            &self,
            _args: Value,
            _ctx: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            Ok(ToolExecutionResult::success("ok"))
        }
    }

    #[test]
    fn test_rbac_policy_denies_by_name() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ShellTool::new()));
        registry.register(Box::new(ReadTool));

        let policy = crate::tools::rbac::ToolPolicy {
            denied_tools: vec!["shell".to_string()],
            ..Default::default()
        };
        let ctx = ToolContext::new("user", "conv1")
            .with_user_context(crate::tools::rbac::UserContext::owner())
            .with_tool_policy(policy);

        let available: Vec<String> = registry
            .get_available(&ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert!(!available.contains(&"shell".to_string()));
        assert!(available.contains(&"read".to_string()));
    }

    #[test]
    fn test_rbac_policy_denies_by_role() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ShellTool::new()));

        let policy = crate::tools::rbac::ToolPolicy {
            required_role: crate::tools::rbac::Role::Admin,
            ..Default::default()
        };
        let admin_ctx = ToolContext::new("admin", "conv1")
            .with_user_context(crate::tools::rbac::UserContext {
                roles: vec![crate::tools::rbac::Role::Admin],
                ..Default::default()
            })
            .with_tool_policy(policy.clone());
        let user_ctx = ToolContext::new("user", "conv1")
            .with_user_context(crate::tools::rbac::UserContext::user())
            .with_tool_policy(policy);

        let admin_available: Vec<String> = registry
            .get_available(&admin_ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();
        let user_available: Vec<String> = registry
            .get_available(&user_ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert!(admin_available.contains(&"shell".to_string()));
        assert!(!user_available.contains(&"shell".to_string()));
    }

    #[test]
    fn test_rbac_policy_denies_by_risk_level() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(HighRiskTool));
        registry.register(Box::new(ReadTool));

        let policy = crate::tools::rbac::ToolPolicy {
            max_risk_level: Some(crate::tools::approval::RiskLevel::Medium),
            ..Default::default()
        };
        let ctx = ToolContext::new("user", "conv1")
            .with_user_context(crate::tools::rbac::UserContext::owner())
            .with_tool_policy(policy);

        let available: Vec<String> = registry
            .get_available(&ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert!(!available.contains(&"high_risk".to_string()));
        assert!(available.contains(&"read".to_string()));
    }

    #[test]
    fn test_rbac_skill_trust_backward_compatible() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ShellTool::new()));
        registry.register(Box::new(ReadTool));
        registry.mark_privileged("shell");

        // No user_context/tool_policy: SkillTrust alone governs availability.
        let community_ctx =
            ToolContext::new("user", "conv1").with_skill_trust(SkillTrust::Community);
        let available: Vec<String> = registry
            .get_available(&community_ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert!(!available.contains(&"shell".to_string()));
        assert!(available.contains(&"read".to_string()));
    }

    // ── Tool gating tests ────────────────────────────────────────────────────

    #[test]
    fn test_plugin_allowlist_gating() {
        let registry = ToolRegistry::new();
        registry.register_dynamic(std::sync::Arc::new(ReadTool));
        registry.register_dynamic(std::sync::Arc::new(PluginTool));
        registry.register_dynamic(std::sync::Arc::new(BlockedPluginTool));

        let ctx =
            ToolContext::new("user", "conv1").with_plugin_allowlist(vec!["allowed__".to_string()]);

        let available: Vec<String> = registry
            .get_available(&ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert!(available.contains(&"read".to_string()));
        assert!(available.contains(&"allowed__foo".to_string()));
        assert!(!available.contains(&"blocked__bar".to_string()));
    }

    struct PluginTool;

    #[async_trait]
    impl Tool for PluginTool {
        fn name(&self) -> &str {
            "allowed__foo"
        }

        fn description(&self) -> &str {
            "An allowed plugin tool"
        }

        fn parameters_schema(&self) -> Value {
            create_schema("Plugin", serde_json::json!({}), Vec::<String>::new())
        }

        async fn execute(
            &self,
            _args: Value,
            _ctx: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            Ok(ToolExecutionResult::success("plugin ok"))
        }
    }

    struct BlockedPluginTool;

    #[async_trait]
    impl Tool for BlockedPluginTool {
        fn name(&self) -> &str {
            "blocked__bar"
        }

        fn description(&self) -> &str {
            "A blocked plugin tool"
        }

        fn parameters_schema(&self) -> Value {
            create_schema("Blocked", serde_json::json!({}), Vec::<String>::new())
        }

        async fn execute(
            &self,
            _args: Value,
            _ctx: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            Ok(ToolExecutionResult::success("blocked"))
        }
    }

    // ── execute()/execute_call() integration tests ───────────────────────────

    #[tokio::test]
    async fn test_execute_simple_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool));

        let ctx = ToolContext::new("user", "conv1");
        let result = registry
            .execute("read", serde_json::json!({"path": "/tmp/test"}), &ctx)
            .await;

        assert!(result.is_some(), "execute should return Some for known tools");
        let inner = result.unwrap();
        assert!(inner.is_ok(), "ReadTool should succeed");
        assert_eq!(inner.unwrap().output, "ok");
    }

    #[tokio::test]
    async fn test_execute_unknown_tool() {
        let registry = ToolRegistry::new();
        let ctx = ToolContext::new("user", "conv1");
        let result = registry
            .execute("nonexistent", serde_json::json!({}), &ctx)
            .await;

        assert!(result.is_none(), "execute should return None for unknown tools");
    }

    #[tokio::test]
    async fn test_execute_blocked_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool));

        // Block the tool by prefix
        registry.deregister_prefix("read");

        let ctx = ToolContext::new("user", "conv1");
        let result = registry
            .execute("read", serde_json::json!({"path": "/tmp/test"}), &ctx)
            .await;

        assert!(result.is_none(), "execute should return None for blocked tools");
    }

    #[tokio::test]
    async fn test_execute_no_cache_skips_cache() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool));

        let ctx = ToolContext::new("user", "conv1");
        // Execute once to warm cache
        let r1 = registry
            .execute("read", serde_json::json!({"path": "/tmp/test"}), &ctx)
            .await;
        assert!(r1.is_some());
        assert!(r1.unwrap().is_ok());

        // execute_no_cache should bypass cache
        let r2 = registry
            .execute_no_cache("read", serde_json::json!({"path": "/tmp/test"}), &ctx)
            .await;
        assert!(r2.is_some());
        assert!(r2.unwrap().is_ok());
    }

    #[tokio::test]
    async fn test_execute_call_simple() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool));

        let call = crate::providers::FunctionCall {
            name: "read".to_string(),
            arguments: "{\"path\": \"/tmp/test\"}".to_string(),
        };
        let ctx = ToolContext::new("user", "conv1");

        let result = registry.execute_call(&call, &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().output, "ok");
    }

    // ── Streaming tool tests ─────────────────────────────────────────────────

    struct StreamingTool;

    #[async_trait]
    impl Tool for StreamingTool {
        fn name(&self) -> &str {
            "streaming_test"
        }

        fn description(&self) -> &str {
            "A tool that streams output"
        }

        fn parameters_schema(&self) -> Value {
            create_schema("Stream", serde_json::json!({}), Vec::<String>::new())
        }

        fn capabilities(&self) -> crate::tools::sdk::ToolCapabilities {
            crate::tools::sdk::ToolCapabilities {
                streaming: true,
                ..Default::default()
            }
        }

        async fn execute(
            &self,
            _args: Value,
            _ctx: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            Ok(ToolExecutionResult::success("final"))
        }
    }

    #[tokio::test]
    async fn test_tool_execute_stream_default() {
        let tool = StreamingTool;
        let ctx = ToolContext::new("user", "conv1");
        let mut stream = tool.execute_stream(serde_json::json!({}), &ctx);

        let chunks: Vec<ToolExecutionChunk> = tokio_stream::StreamExt::collect(&mut stream).await;
        assert_eq!(chunks.len(), 1);
        assert!(matches!(chunks[0], ToolExecutionChunk::Output(ref s) if s == "final"));
    }

    #[tokio::test]
    async fn test_registry_execute_call_streaming() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(StreamingTool));

        let call = crate::providers::FunctionCall {
            name: "streaming_test".to_string(),
            arguments: "{}".to_string(),
        };
        let ctx = ToolContext::new("user", "conv1");

        let mut chunks = Vec::new();
        let result = registry
            .execute_call_streaming(&call, &ctx, |chunk| {
                chunks.push(chunk.clone());
                async move {}
            })
            .await;

        assert!(result.is_ok(), "streaming execution should succeed");
        let result = result.unwrap();
        assert_eq!(result.output, "final");
        assert!(chunks
            .iter()
            .any(|c| matches!(c, ToolExecutionChunk::Output(s) if s == "final")));
    }

    #[test]
    fn test_tool_execution_chunk_serialization() {
        let chunk = ToolExecutionChunk::Output("hello".to_string());
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("\"kind\":\"output\""));
        assert!(json.contains("\"payload\":\"hello\""));

        let decoded: ToolExecutionChunk = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, ToolExecutionChunk::Output(s) if s == "hello"));
    }
}
