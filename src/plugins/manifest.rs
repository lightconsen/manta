//! Plugin Manifest Definition
//!
//! Defines the structure of plugin.json/manifest.json files

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Plugin manifest - describes a plugin's metadata and capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin identifier (e.g., "com.example.my-plugin")
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Plugin version (semver)
    pub version: String,
    /// Plugin description
    pub description: String,
    /// Plugin author
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Main entry point (WASM file)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main: Option<String>,
    /// Capabilities this plugin provides
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<PluginCapability>>,
    /// Permissions this plugin requires
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<PluginPermission>>,
    /// Default configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
}

/// Plugin capability - what the plugin can do
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginCapability {
    /// Provides custom tools
    Tools {
        /// List of tools provided
        tools: Vec<PluginTool>,
    },
    /// Provides a channel implementation
    Channel {
        /// Channel type identifier
        channel_type: String,
        /// Channel display name
        name: String,
    },
    /// Provides hooks for extending behavior
    Hooks {
        /// List of hooks implemented
        hooks: Vec<String>,
    },
    /// Provides custom commands
    Commands {
        /// List of commands
        commands: Vec<PluginCommand>,
    },
}

/// Tool definition from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginTool {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// JSON schema for parameters
    pub parameters: serde_json::Value,
    /// Whether the tool is dangerous (requires confirmation)
    #[serde(default)]
    pub dangerous: bool,
}

/// Command definition from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCommand {
    /// Command name
    pub name: String,
    /// Command description
    pub description: String,
    /// Arguments
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<PluginArg>>,
}

/// Command argument
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginArg {
    /// Argument name
    pub name: String,
    /// Argument description
    pub description: String,
    /// Whether argument is required
    #[serde(default)]
    pub required: bool,
    /// Default value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Plugin permission - what the plugin is allowed to do
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginPermission {
    /// Access to filesystem
    Filesystem { paths: Vec<String> },
    /// Access to network
    Network { hosts: Vec<String> },
    /// Access to environment variables
    Env { vars: Vec<String> },
    /// Access to system commands
    System { commands: Vec<String> },
    /// Access to memory/store
    Memory,
    /// Access to configuration
    Config,
}

impl PluginManifest {
    /// Create a minimal manifest for testing
    pub fn minimal(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            version: "0.1.0".to_string(),
            description: "A Manta plugin".to_string(),
            author: None,
            main: None,
            capabilities: None,
            permissions: None,
            config: None,
        }
    }

    /// Check if plugin has a specific capability
    pub fn has_capability(&self, capability_type: &str) -> bool {
        if let Some(ref capabilities) = self.capabilities {
            capabilities.iter().any(|c| {
                let t = match c {
                    PluginCapability::Tools { .. } => "tools",
                    PluginCapability::Channel { .. } => "channel",
                    PluginCapability::Hooks { .. } => "hooks",
                    PluginCapability::Commands { .. } => "commands",
                };
                t == capability_type
            })
        } else {
            false
        }
    }

    /// Get tools if available
    pub fn get_tools(&self) -> Vec<&PluginTool> {
        if let Some(ref capabilities) = self.capabilities {
            capabilities
                .iter()
                .filter_map(|c| match c {
                    PluginCapability::Tools { tools } => Some(tools.iter()),
                    _ => None,
                })
                .flatten()
                .collect()
        } else {
            vec![]
        }
    }
}
