//! Tool SDK Extension Layer
//!
//! Extends the core tool registry with tool packs, capability schemas,
//! and dynamic registration hooks for plugin-provided tools.
//!
//! Design matches OpenClaw's tool extension architecture.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Capabilities advertised by a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCapabilities {
    /// Whether this tool requires human approval.
    pub requires_approval: bool,
    /// Whether this tool can run in a sandbox.
    pub sandboxed: bool,
    /// Whether this tool supports streaming output.
    pub streaming: bool,
    /// Risk level classification.
    pub risk_level: crate::tools::approval::RiskLevel,
    /// Categories (e.g. ["file", "system", "network"]).
    pub categories: Vec<String>,
}

impl Default for ToolCapabilities {
    fn default() -> Self {
        Self {
            requires_approval: false,
            sandboxed: false,
            streaming: false,
            risk_level: crate::tools::approval::RiskLevel::Low,
            categories: vec![],
        }
    }
}

/// Metadata for a registered tool.
#[derive(Debug, Clone)]
pub struct ToolMetadata {
    pub name: String,
    pub description: String,
    pub capabilities: ToolCapabilities,
    pub parameter_schema: serde_json::Value,
}

/// A "tool pack" — a bundle of related tools shipped as a unit.
///
/// OpenClaw uses tool packs to group tools by domain (e.g. "filesystem",
/// "web", "system").
pub struct ToolPack {
    pub name: String,
    pub version: String,
    pub description: String,
    pub tools: Vec<String>,
}

/// Tool SDK — entry point for dynamic tool registration and discovery.
pub struct ToolSdk {
    packs: HashMap<String, ToolPack>,
}

impl ToolSdk {
    pub fn new() -> Self {
        Self { packs: HashMap::new() }
    }

    pub fn register_pack(&mut self, pack: ToolPack) {
        self.packs.insert(pack.name.clone(), pack);
    }

    pub fn list_packs(&self) -> Vec<&ToolPack> {
        self.packs.values().collect()
    }

    pub fn get_pack(&self, name: &str) -> Option<&ToolPack> {
        self.packs.get(name)
    }

    /// Synchronize tool packs from the ToolRegistry.
    ///
    /// Creates a tool pack containing all tools currently registered
    /// in the tool registry (both static and dynamic).
    pub fn sync_from_tool_registry(&mut self, tool_registry: &crate::tools::ToolRegistry) {
        let tools = tool_registry.list();
        if tools.is_empty() {
            return;
        }
        self.register_pack(ToolPack {
            name: "default_tools".to_string(),
            version: "1.0".to_string(),
            description: "Tools synchronized from ToolRegistry".to_string(),
            tools,
        });
    }
}

impl Default for ToolSdk {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_capabilities_default() {
        let caps = ToolCapabilities::default();
        assert!(!caps.requires_approval);
        assert!(!caps.sandboxed);
    }

    #[test]
    fn test_tool_pack() {
        let mut sdk = ToolSdk::new();
        sdk.register_pack(ToolPack {
            name: "filesystem".to_string(),
            version: "1.0".to_string(),
            description: "File and directory operations".to_string(),
            tools: vec!["read_file".to_string(), "write_file".to_string()],
        });
        assert_eq!(sdk.list_packs().len(), 1);
        assert!(sdk.get_pack("filesystem").is_some());
    }
}
