//! MCP (Model Context Protocol) Integration
//!
//! This module implements a client for the Model Context Protocol,
//! allowing Syscity to connect to MCP servers and use their tools.
//!
//! Supported transports:
//! - `stdio` – spawn a subprocess and communicate over stdin/stdout
//! - `sse` – connect to an HTTP server via Server-Sent Events
//! - `streamable_http` – POST requests with SSE response bodies

mod client;
mod config;
mod manager;
mod oauth;
mod tools;
mod types;

/// The default MCP presets embedded in the binary.
pub const DEFAULT_PRESETS_TOML: &str = include_str!("presets.toml");

// ─────────────────────────────────────────────
// Re-exports — preserve the `crate::mcp::*` API surface
// ─────────────────────────────────────────────

pub use client::McpClient;
pub use config::{McpConfig, McpServerConfig, McpSettings, McpTransport};
pub use manager::{McpConnectionMeta, McpManager};
pub use oauth::{mcp_tokens_dir, token_path_for, OAuthManager, OAuthTokens};
pub(crate) use oauth::{OAuthCommand, OAuthManagerActor};
pub use tools::{McpConnectionTool, McpPromptTool, McpToolWrapper};
pub use types::{
    McpEvent, McpGetPromptResult, McpHealth, McpHealthStatus, McpNotification, McpPrompt,
    McpPromptArgument, McpPromptMessage, McpPromptsCapability, McpResource, McpResourceContent,
    McpResourcesCapability, McpSamplingMessage, McpSamplingResult, McpServerCapabilities,
    McpToolDefinition, McpToolsCapability,
};
pub(crate) use types::{McpInitializeResult, McpRequest, McpResponse, McpServerInfo};

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::tools::Tool;

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
    fn test_merged_env_does_not_expand_literal_tokens() {
        // A literal stored token that begins with `$` must NOT be run through
        // env-var expansion (regression for the env-store feature).
        std::env::set_var("HOME", "/fake/home");
        let mut env = HashMap::new();
        env.insert("REF".to_string(), "$HOME".to_string());
        let mut resolved_env = HashMap::new();
        resolved_env.insert("TOKEN".to_string(), "$HOME_literal".to_string());

        let config = McpServerConfig {
            env,
            resolved_env,
            ..Default::default()
        };
        let merged = McpClient::merged_env(&config);
        assert_eq!(merged["REF"], "/fake/home");
        assert_eq!(merged["TOKEN"], "$HOME_literal");
        std::env::remove_var("HOME");
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
