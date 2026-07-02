//! Plugin Manifest Definition
//!
//! Defines the structure of plugin.json/manifest.json files

use std::collections::HashMap;

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
    /// Minimum required Syscity version (semver constraint, e.g. ">=0.2.0")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub syscity_version: Option<String>,
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
    /// Activation triggers (for lazy loading)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triggers: Option<Vec<super::activation::PluginTrigger>>,
    /// Plugin dependencies (plugin IDs -> version constraints)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<HashMap<String, String>>,
    /// Repository URL for the plugin source code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Registry URL where this plugin was published
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// Base64-encoded ed25519 signature over canonical manifest fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Base64-encoded ed25519 public key of the signer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer_public_key: Option<String>,
    /// External resources that must be downloaded at install/load time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_resources: Option<Vec<ExternalResource>>,
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
    /// Provides LLM model entries for the catalog
    Models {
        /// List of models provided by this plugin
        models: Vec<PluginModelEntry>,
    },
    /// Provides a custom LLM provider implementation
    Provider {
        /// Provider name identifier
        name: String,
        /// Default model for this provider
        default_model: String,
        /// Stream family for this provider
        stream_family: String,
        /// Whether this provider supports tool calling
        supports_tools: bool,
        /// Max context window size
        max_context: usize,
    },
}

impl PluginCapability {
    /// Return the variant name as a &str (for use with `has_capability`).
    pub fn variant_name(&self) -> &'static str {
        match self {
            PluginCapability::Tools { .. } => "tools",
            PluginCapability::Channel { .. } => "channel",
            PluginCapability::Hooks { .. } => "hooks",
            PluginCapability::Commands { .. } => "commands",
            PluginCapability::Models { .. } => "models",
            PluginCapability::Provider { .. } => "provider",
        }
    }
}

/// A model entry declared by a plugin for dynamic model discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginModelEntry {
    /// Provider-specific model ID
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Provider name (e.g. "openai", "anthropic", or a custom plugin provider)
    pub provider: String,
    /// Context window size in tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<usize>,
    /// Whether the model supports vision / image input
    #[serde(default)]
    pub supports_vision: bool,
    /// Whether the model supports tool calling
    #[serde(default)]
    pub supports_tools: bool,
    /// Whether the model supports reasoning / thinking
    #[serde(default)]
    pub supports_reasoning: bool,
    /// Supported input modalities
    #[serde(default)]
    pub input_modalities: Vec<String>,
    /// Pricing: input cost per 1K tokens (USD)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_cost_per_1k: Option<f64>,
    /// Pricing: output cost per 1K tokens (USD)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_cost_per_1k: Option<f64>,
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

/// External resource that a plugin requires at runtime
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalResource {
    /// URL to download the resource from
    pub url: String,
    /// Relative path within the plugin directory to place the resource
    pub path: String,
    /// Optional SHA-256 checksum for verification
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum_sha256: Option<String>,
    /// Whether this resource is required (load fails if missing)
    pub required: bool,
}

impl PluginManifest {
    /// Create a minimal manifest for testing
    pub fn minimal(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            version: "0.1.0".to_string(),
            syscity_version: None,
            description: "A Syscity plugin".to_string(),
            author: None,
            main: None,
            capabilities: None,
            permissions: None,
            config: None,
            triggers: None,
            dependencies: None,
            repository: None,
            registry: None,
            signature: None,
            signer_public_key: None,
            external_resources: None,
        }
    }

    /// Check if plugin has a specific capability
    pub fn has_capability(&self, capability_type: &str) -> bool {
        if let Some(ref capabilities) = self.capabilities {
            capabilities
                .iter()
                .any(|c| c.variant_name() == capability_type)
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

    /// Get model entries if available
    pub fn get_models(&self) -> Vec<&PluginModelEntry> {
        if let Some(ref capabilities) = self.capabilities {
            capabilities
                .iter()
                .filter_map(|c| match c {
                    PluginCapability::Models { models } => Some(models.iter()),
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
            capabilities: Some(vec![PluginCapability::Tools { tools: vec![tool.clone()] }]),
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
            syscity_version: None,
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
            triggers: None,
            dependencies: None,
            repository: None,
            registry: None,
            signature: None,
            signer_public_key: None,
            external_resources: None,
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
