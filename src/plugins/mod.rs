//! Plugin System for Manta
//!
//! Provides runtime extensibility similar to OpenClaw's plugin SDK:
//! - WASM-based sandboxed plugins
//! - Tool registration from plugins
//! - Channel plugins
//! - Hooks system for extending behavior
//! - Hot loading/unloading

pub mod hooks;
pub mod manifest;
pub mod runtime;

pub use hooks::{
    HookExecutionResult, HookHandler, HookHandlerBuilder, HookPayload, HookRegistry,
    HookResult, HookType,
};
pub use manifest::{
    PluginArg, PluginCapability, PluginCommand, PluginManifest, PluginPermission, PluginTool,
};
pub use runtime::{PluginInstance, PluginRuntime};

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Plugin manager - high-level interface for plugin operations
pub struct PluginManager {
    runtime: Arc<PluginRuntime>,
    hook_registry: Arc<HookRegistry>,
    plugins_dir: PathBuf,
    auto_load: bool,
}

impl PluginManager {
    /// Create a new plugin manager
    pub async fn new(plugins_dir: PathBuf) -> crate::Result<Self> {
        let runtime = Arc::new(PluginRuntime::new()?);
        let hook_registry = Arc::new(HookRegistry::new());

        // Ensure plugins directory exists
        tokio::fs::create_dir_all(&plugins_dir).await.ok();

        Ok(Self {
            runtime,
            hook_registry,
            plugins_dir,
            auto_load: true,
        })
    }

    /// Initialize and load all plugins
    pub async fn initialize(&self) -> crate::Result<usize> {
        info!("Initializing plugin manager...");

        let mut entries = tokio::fs::read_dir(&self.plugins_dir).await?;
        let mut count = 0;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            if path.is_dir() {
                let manifest_path = path.join("plugin.json");
                if manifest_path.exists() {
                    match self.runtime.load_plugin(&path).await {
                        Ok(plugin_id) => {
                            debug!("Auto-loaded plugin '{}'", plugin_id);
                            count += 1;
                        }
                        Err(e) => {
                            warn!("Failed to load plugin from {:?}: {}", path, e);
                        }
                    }
                }
            }
        }

        info!("Loaded {} plugin(s)", count);
        Ok(count)
    }

    /// Load a plugin from a directory
    pub async fn load_plugin(&self, path: &std::path::Path) -> crate::Result<String> {
        self.runtime.load_plugin(path).await
    }

    /// Unload a plugin
    pub async fn unload_plugin(&self, plugin_id: &str) -> crate::Result<bool> {
        // Unregister hooks first
        self.hook_registry.unregister_plugin(plugin_id).await;

        // Unload the plugin
        self.runtime.unload_plugin(plugin_id).await
    }

    /// Get a plugin instance
    pub async fn get_plugin(&self, plugin_id: &str) -> Option<PluginInstance> {
        self.runtime.get_plugin(plugin_id).await
    }

    /// List all plugins
    pub async fn list_plugins(&self) -> Vec<PluginInstance> {
        self.runtime.list_plugins().await
    }

    /// Enable/disable a plugin
    pub async fn set_enabled(&self, plugin_id: &str, enabled: bool) -> crate::Result<()> {
        self.runtime.set_enabled(plugin_id, enabled).await
    }

    /// Get the hook registry
    pub fn hook_registry(&self) -> &Arc<HookRegistry> {
        &self.hook_registry
    }

    /// Get the plugin runtime
    pub fn runtime(&self) -> &Arc<PluginRuntime> {
        &self.runtime
    }

    /// Execute a hook
    pub async fn execute_hook(
        &self,
        hook_type: HookType,
        payload: HookPayload,
    ) -> HookExecutionResult {
        self.hook_registry.execute(hook_type, payload).await
    }

    /// Register a hook handler
    pub async fn register_hook(&self, handler: HookHandler) {
        self.hook_registry.register(handler).await;
    }

    /// Shutdown all plugins
    pub async fn shutdown(&self) -> crate::Result<()> {
        info!("Shutting down plugin manager...");
        self.runtime.shutdown().await
    }

    /// Create a sample plugin template
    pub async fn create_template(
        &self,
        name: &str,
        description: &str,
    ) -> crate::Result<PathBuf> {
        let plugin_dir = self.plugins_dir.join(name);
        tokio::fs::create_dir_all(&plugin_dir).await?;

        // Create manifest
        let manifest = PluginManifest {
            id: format!("com.example.{}", name),
            name: name.to_string(),
            version: "0.1.0".to_string(),
            description: description.to_string(),
            author: Some("Your Name".to_string()),
            main: None,
            capabilities: Some(vec![PluginCapability::Hooks {
                hooks: vec!["before_tool_execute".to_string()],
            }]),
            permissions: Some(vec![PluginPermission::Memory]),
            config: Some(serde_json::json!({
                "example_setting": "value"
            })),
        };

        let manifest_json = serde_json::to_string_pretty(&manifest)?;
        tokio::fs::write(plugin_dir.join("plugin.json"), manifest_json).await?;

        // Create config.json
        let config = serde_json::json!({
            "example_setting": "value"
        });
        tokio::fs::write(
            plugin_dir.join("config.json"),
            serde_json::to_string_pretty(&config)?,
        )
        .await?;

        // Create README
        let readme = format!(
            r#"# {}

{}

## Installation

Place this directory in `{}`

## Configuration

Edit `config.json` to customize settings.

## Capabilities

- Hooks: before_tool_execute

## Permissions

- Memory
"#,
            name,
            description,
            self.plugins_dir.display()
        );
        tokio::fs::write(plugin_dir.join("README.md"), readme).await?;

        info!("Created plugin template at {:?}", plugin_dir);
        Ok(plugin_dir)
    }
}

/// Plugin tool wrapper - adapts plugin tools to Manta's Tool trait
use crate::tools::{Tool, ToolContext, ToolExecutionResult};

pub struct PluginToolWrapper {
    plugin_id: String,
    tool_name: String,
    description: String,
    parameters: serde_json::Value,
    runtime: Arc<PluginRuntime>,
}

impl PluginToolWrapper {
    pub fn new(
        plugin_id: String,
        tool: &PluginTool,
        runtime: Arc<PluginRuntime>,
    ) -> Self {
        Self {
            plugin_id,
            tool_name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
            runtime,
        }
    }
}

#[async_trait::async_trait]
impl Tool for PluginToolWrapper {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();

        let result = self
            .runtime
            .call_tool(&self.plugin_id, &self.tool_name, args)
            .await;

        match result {
            Ok(output) => Ok(ToolExecutionResult {
                success: true,
                output: output.to_string(),
                error: None,
                data: Some(output),
                execution_time: start.elapsed(),
            }),
            Err(e) => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
                data: None,
                execution_time: start.elapsed(),
            }),
        }
    }
}
