//! Tool abstractions for Manta
//!
//! Tools are capabilities that the AI assistant can use to interact
//! with the world (execute shell commands, read files, search the web, etc.).

use crate::providers::{FunctionCall, FunctionDefinition, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

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

/// The execution context for a tool
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// The user ID executing the tool
    pub user_id: String,
    /// The conversation ID
    pub conversation_id: String,
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
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            conversation_id: String::new(),
            working_directory: std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from(".")),
            environment: std::env::vars().collect(),
            timeout: Duration::from_secs(30),
            allowed_paths: Vec::new(),
            allowed_commands: Vec::new(),
            sandboxed: false,
            memory_limit: None,
            cpu_limit: None,
            fd_limit: None,
            process_limit: None,
        }
    }
}

impl ToolContext {
    /// Create a new tool context
    pub fn new(user_id: impl Into<String>, conversation_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            conversation_id: conversation_id.into(),
            ..Default::default()
        }
    }

    /// Set the working directory
    pub fn with_working_dir(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.working_directory = path.into();
        self
    }

    /// Set the timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Add an allowed path
    pub fn allow_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.allowed_paths.push(path.into());
        self
    }

    /// Add an allowed command
    pub fn allow_command(mut self, command: impl Into<String>) -> Self {
        self.allowed_commands.push(command.into());
        self
    }

    /// Set sandboxed mode
    pub fn sandboxed(mut self, sandboxed: bool) -> Self {
        self.sandboxed = sandboxed;
        self
    }

    /// Set memory limit in bytes (only effective when sandboxed)
    pub fn with_memory_limit(mut self, bytes: usize) -> Self {
        self.memory_limit = Some(bytes);
        self
    }

    /// Set CPU time limit in seconds (only effective when sandboxed)
    pub fn with_cpu_limit(mut self, seconds: u64) -> Self {
        self.cpu_limit = Some(seconds);
        self
    }

    /// Set file descriptor limit (only effective when sandboxed)
    pub fn with_fd_limit(mut self, count: u64) -> Self {
        self.fd_limit = Some(count);
        self
    }

    /// Set process limit for preventing fork bombs (only effective when sandboxed)
    pub fn with_process_limit(mut self, count: u64) -> Self {
        self.process_limit = Some(count);
        self
    }

    /// Apply resource limits to the current process (Unix only)
    /// This should be called in a pre_exec hook before spawning the child process
    #[cfg(unix)]
    pub fn apply_resource_limits(&self) -> std::io::Result<()> {
        use std::io;

        // Only apply limits if sandboxed
        if !self.sandboxed {
            return Ok(());
        }

        // Apply memory limit
        if let Some(memory_limit) = self.memory_limit {
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
        if let Some(cpu_limit) = self.cpu_limit {
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
        if let Some(fd_limit) = self.fd_limit {
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
        if let Some(process_limit) = self.process_limit {
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
        if !self.sandboxed {
            return "No sandbox (no resource limits)".to_string();
        }

        let mut parts = vec!["Sandbox active".to_string()];

        if let Some(mem) = self.memory_limit {
            parts.push(format!("Memory: {} MB", mem / 1024 / 1024));
        }
        if let Some(cpu) = self.cpu_limit {
            parts.push(format!("CPU: {}s", cpu));
        }
        if let Some(fd) = self.fd_limit {
            parts.push(format!("FDs: {}", fd));
        }
        if let Some(proc) = self.process_limit {
            parts.push(format!("Processes: {}", proc));
        }

        if parts.len() == 1 {
            parts.push("No specific limits set".to_string());
        }

        parts.join(" | ")
    }

    /// Check if a path is allowed
    pub fn is_path_allowed(&self, path: &std::path::Path) -> bool {
        if self.allowed_paths.is_empty() {
            return true;
        }
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.allowed_paths.iter().any(|allowed| {
            allowed.canonicalize().map_or(false, |a| path.starts_with(&a))
        })
    }

    /// Check if a command is allowed
    pub fn is_command_allowed(&self, command: &str) -> bool {
        if self.allowed_commands.is_empty() {
            return true;
        }
        let cmd = command.split_whitespace().next().unwrap_or(command);
        self.allowed_commands.iter().any(|allowed| allowed == cmd)
    }
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
pub trait Tool: Send + Sync {
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

    /// Check if this tool is available in the given context
    fn is_available(&self, _context: &ToolContext) -> bool {
        true
    }

    /// Get the timeout for this tool (defaults to context timeout)
    fn timeout(&self, context: &ToolContext) -> Duration {
        context.timeout
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

pub mod code_exec;
pub mod cron_tool;
pub mod delegate_tool;
pub mod file;
pub mod grep;
pub mod mcp;
pub mod memory;
pub mod shell;
pub mod time;
pub mod todo_tool;
pub mod web;

pub use code_exec::CodeExecutionTool;
pub use cron_tool::CronTool;
pub use delegate_tool::DelegateTool;
pub use file::{FileEditTool, FileReadTool, FileWriteTool, GlobTool};
pub use grep::GrepTool;
pub use mcp::McpConnectionTool;
pub use memory::MemoryTool;
pub use shell::ShellTool;
pub use time::TimeTool;
pub use todo_tool::TodoTool;
pub use web::{WebFetchTool, WebSearchTool};

/// Cached tool result entry
#[derive(Debug, Clone)]
struct CacheEntry {
    result: ToolExecutionResult,
    timestamp: std::time::Instant,
}

/// Registry of tools with optional caching
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, BoxedTool>,
    cache: std::sync::Mutex<HashMap<String, CacheEntry>>,
    cache_ttl: Option<Duration>,
    cache_enabled: bool,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            cache: std::sync::Mutex::new(HashMap::new()),
            cache_ttl: None,
            cache_enabled: false,
        }
    }

    /// Create a new registry with caching enabled
    pub fn with_cache(ttl: Duration) -> Self {
        Self {
            tools: HashMap::new(),
            cache: std::sync::Mutex::new(HashMap::new()),
            cache_ttl: Some(ttl),
            cache_enabled: true,
        }
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

        let cache = self.cache.lock().ok()?;
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
            cache.insert(key, CacheEntry {
                result,
                timestamp: std::time::Instant::now(),
            });
        }
    }

    /// Register a tool
    pub fn register(&mut self, tool: BoxedTool) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// List available tool names
    pub fn list(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a tool exists
    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get all tools as function definitions
    pub fn get_definitions(&self) -> Vec<FunctionDefinition> {
        self.tools
            .values()
            .map(|t| t.to_function_definition())
            .collect()
    }

    /// Get all available tools for a given context
    pub fn get_available(&self, context: &ToolContext) -> Vec<FunctionDefinition> {
        self.tools
            .values()
            .filter(|t| t.is_available(context))
            .map(|t| t.to_function_definition())
            .collect()
    }

    /// Execute a tool by name with optional caching
    pub async fn execute(
        &self,
        name: &str,
        args: Value,
        context: &ToolContext,
    ) -> Option<crate::Result<ToolExecutionResult>> {
        // Check cache first
        let cache_key = Self::cache_key(name, &args);
        if let Some(cached_result) = self.get_cached(&cache_key) {
            tracing::debug!("Cache hit for tool: {}", name);
            return Some(Ok(cached_result));
        }

        let tool = self.get(name)?;
        let result = tool.execute(args, context).await;

        // Cache successful results
        if let Ok(ref exec_result) = result {
            self.store_cached(cache_key, exec_result.clone());
        }

        Some(result)
    }

    /// Execute a tool by name without caching
    pub async fn execute_no_cache(
        &self,
        name: &str,
        args: Value,
        context: &ToolContext,
    ) -> Option<crate::Result<ToolExecutionResult>> {
        let tool = self.get(name)?;
        Some(tool.execute(args, context).await)
    }

    /// Execute a function call from an LLM
    pub async fn execute_call(
        &self,
        call: &FunctionCall,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let tool = self
            .get(&call.name)
            .ok_or_else(|| crate::error::MantaError::Validation(format!(
                "Unknown tool: {}",
                call.name
            )))?;

        let args: Value = serde_json::from_str(&call.arguments).map_err(|e| {
            crate::error::MantaError::Validation(format!(
                "Invalid arguments for tool {}: {}",
                call.name, e
            ))
        })?;

        tool.execute(args, context).await
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
            return Err(ToolValidationError::InvalidName(
                format!("Tool name '{}' must be between 2 and 64 characters", name)
            ));
        }

        // Check characters (alphanumeric, underscore, hyphen only)
        if !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            return Err(ToolValidationError::InvalidName(
                format!("Tool name '{}' contains invalid characters. Use alphanumeric, underscore, or hyphen only", name)
            ));
        }

        // Check doesn't start with number
        if name.chars().next().map(|c| c.is_numeric()).unwrap_or(false) {
            return Err(ToolValidationError::InvalidName(
                format!("Tool name '{}' cannot start with a number", name)
            ));
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
                "Schema must have type 'object'".to_string()
            ));
        }

        if schema.get("properties").is_none() {
            return Err(ToolValidationError::InvalidSchema(
                "Schema must have 'properties' field".to_string()
            ));
        }

        Ok(())
    }

    fn validate_input(&self, tool_name: &str, args: &Value) -> Result<(), ToolValidationError> {
        // Basic JSON structure validation
        if !args.is_object() && !args.is_null() {
            return Err(ToolValidationError::InvalidInput(
                format!("Tool '{}' arguments must be a JSON object", tool_name)
            ));
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
                return Err(ToolValidationError::SecurityViolation(
                    format!("Path traversal attempt detected: {}", pattern)
                ));
            }
        }

        // Check for double slashes (can be used in some path traversal attacks)
        if value.contains("//") || value.contains("\\\\") {
            return Err(ToolValidationError::SecurityViolation(
                "Suspicious path pattern detected".to_string()
            ));
        }

        Ok(())
    }

    /// Check for command injection attempts
    fn check_command_injection(&self, value: &str) -> Result<(), ToolValidationError> {
        let dangerous_chars = [';', '&', '|', '$', '`', '\n', '\r'];

        for ch in &dangerous_chars {
            if value.contains(*ch) {
                return Err(ToolValidationError::SecurityViolation(
                    format!("Command injection attempt detected: contains '{}'", ch)
                ));
            }
        }

        // Check for command substitution patterns
        if value.contains("$(") || value.contains("${") {
            return Err(ToolValidationError::SecurityViolation(
                "Command substitution pattern detected".to_string()
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
                "Tool description must be at least 10 characters".to_string()
            ));
        }

        Ok(())
    }

    fn validate_input(&self, _tool_name: &str, args: &Value) -> Result<(), ToolValidationError> {
        // Recursively check all string values for security issues
        fn check_value(value: &Value, validator: &SecurityValidator) -> Result<(), ToolValidationError> {
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
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.registry.get(name)
    }

    /// List available tool names
    pub fn list(&self) -> Vec<&str> {
        self.registry.list()
    }

    /// Check if a tool exists
    pub fn has(&self, name: &str) -> bool {
        self.registry.has(name)
    }

    /// Get tool descriptions
    pub fn get_descriptions(&self) -> HashMap<String, String> {
        self.registry.list()
            .into_iter()
            .filter_map(|name| {
                self.registry.get(name)
                    .map(|t| (name.to_string(), t.description().to_string()))
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
            return Some(Err(crate::error::MantaError::Validation(e.to_string())));
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
        assert_eq!(ctx.timeout, Duration::from_secs(60));
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
        let mut registry = ToolRegistry::new();
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
        use crate::providers::FunctionDefinition;

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
            async fn execute(&self, _args: Value, _ctx: &ToolContext) -> crate::Result<ToolExecutionResult> {
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
            async fn execute(&self, _args: Value, _ctx: &ToolContext) -> crate::Result<ToolExecutionResult> {
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
        assert!(validator.check_path_traversal("/home/user/file.txt").is_ok());
        assert!(validator.check_path_traversal("./file.txt").is_ok());

        // Invalid paths with traversal
        assert!(validator.check_path_traversal("../etc/passwd").is_err());
        assert!(validator.check_path_traversal("foo/../../../etc/passwd").is_err());
    }

    #[test]
    fn test_security_validator_command_injection() {
        let validator = SecurityValidator;

        // Valid commands
        assert!(validator.check_command_injection("ls -la").is_ok());
        assert!(validator.check_command_injection("cat file.txt").is_ok());

        // Invalid commands with injection
        assert!(validator.check_command_injection("ls; rm -rf /").is_err());
        assert!(validator.check_command_injection("cat file | grep test").is_err());
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
}
