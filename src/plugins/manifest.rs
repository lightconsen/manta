//! Plugin Manifest Definition
//!
//! Defines the structure of plugin.json/manifest.json files

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_manifest_minimal() {
        let manifest = PluginManifest::minimal("com.test.plugin", "Test Plugin");
        assert_eq!(manifest.id, "com.test.plugin");
        assert_eq!(manifest.name, "Test Plugin");
        assert_eq!(manifest.version, "0.1.0");
        assert!(manifest.capabilities.is_none());
        assert!(manifest.permissions.is_none());
    }

    #[test]
    fn test_has_capability_no_capabilities() {
        let manifest = PluginManifest::minimal("test", "Test");
        assert!(!manifest.has_capability("tools"));
        assert!(!manifest.has_capability("channel"));
    }

    #[test]
    fn test_has_capability_with_tools() {
        let manifest = PluginManifest {
            capabilities: Some(vec![PluginCapability::Tools {
                tools: vec![PluginTool {
                    name: "grep".to_string(),
                    description: "Search files".to_string(),
                    parameters: serde_json::json!({}),
                    dangerous: false,
                }],
            }]),
            ..PluginManifest::minimal("test", "Test")
        };
        assert!(manifest.has_capability("tools"));
        assert!(!manifest.has_capability("hooks"));
    }

    #[test]
    fn test_has_capability_all_types() {
        let manifest = PluginManifest {
            capabilities: Some(vec![
                PluginCapability::Channel {
                    channel_type: "slack".to_string(),
                    name: "Slack".to_string(),
                },
                PluginCapability::Hooks {
                    hooks: vec!["before_tool".to_string()],
                },
                PluginCapability::Commands {
                    commands: vec![PluginCommand {
                        name: "status".to_string(),
                        description: "Show status".to_string(),
                        args: None,
                    }],
                },
            ]),
            ..PluginManifest::minimal("test", "Test")
        };
        assert!(manifest.has_capability("channel"));
        assert!(manifest.has_capability("hooks"));
        assert!(manifest.has_capability("commands"));
        assert!(!manifest.has_capability("tools"));
    }

    #[test]
    fn test_get_tools_empty() {
        let manifest = PluginManifest::minimal("test", "Test");
        assert!(manifest.get_tools().is_empty());
    }

    #[test]
    fn test_get_tools_returns_tools() {
        let tool = PluginTool {
            name: "grep".to_string(),
            description: "Search".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            dangerous: true,
        };
        let manifest = PluginManifest {
            capabilities: Some(vec![PluginCapability::Tools {
                tools: vec![tool.clone()],
            }]),
            ..PluginManifest::minimal("test", "Test")
        };
        let tools = manifest.get_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "grep");
        assert!(tools[0].dangerous);
    }

    #[test]
    fn test_plugin_manifest_serde_roundtrip() {
        let manifest = PluginManifest {
            id: "com.example.plugin".to_string(),
            name: "Example".to_string(),
            version: "1.0.0".to_string(),
            description: "An example plugin".to_string(),
            author: Some("Alice".to_string()),
            main: Some("plugin.wasm".to_string()),
            capabilities: Some(vec![PluginCapability::Tools {
                tools: vec![PluginTool {
                    name: "echo".to_string(),
                    description: "Echo".to_string(),
                    parameters: serde_json::json!({}),
                    dangerous: false,
                }],
            }]),
            permissions: Some(vec![PluginPermission::Memory]),
            config: Some(serde_json::json!({"timeout": 30})),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let decoded: PluginManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, manifest.id);
        assert_eq!(decoded.name, manifest.name);
        assert_eq!(decoded.version, manifest.version);
    }

    #[test]
    fn test_plugin_capability_serde() {
        let cap = PluginCapability::Tools {
            tools: vec![PluginTool {
                name: "t1".to_string(),
                description: "desc".to_string(),
                parameters: serde_json::json!({}),
                dangerous: false,
            }],
        };
        let json = serde_json::to_value(&cap).unwrap();
        assert_eq!(json["type"], "tools");

        let decoded: PluginCapability = serde_json::from_value(json).unwrap();
        assert!(matches!(decoded, PluginCapability::Tools { .. }));
    }

    #[test]
    fn test_plugin_permission_serde() {
        let perm = PluginPermission::Filesystem {
            paths: vec!["/tmp".to_string()],
        };
        let json = serde_json::to_value(&perm).unwrap();
        let decoded: PluginPermission = serde_json::from_value(json).unwrap();
        assert!(
            matches!(decoded, PluginPermission::Filesystem { paths } if paths == vec!["/tmp".to_string()])
        );
    }

}
