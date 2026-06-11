//! Plugin System for Syscity
//!
//! Provides runtime extensibility:
//! - WASM-based sandboxed plugins
//! - Tool registration from plugins
//! - Channel plugins
//! - Hooks system for extending behavior
//! - Hot loading/unloading

pub mod hooks;
pub mod manifest;
pub mod provider_extension;
pub mod runtime;

pub use hooks::{
    HookExecutionResult, HookHandler, HookHandlerBuilder, HookPayload, HookRegistry, HookResult,
    HookType,
};
pub use manifest::{
    PluginArg, PluginCapability, PluginCommand, PluginManifest, PluginPermission, PluginTool,
};
pub use provider_extension::{PluginProvider, PluginProviderRegistry};
pub use runtime::{PluginInstance, PluginRuntime};

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::tools::ToolRegistry;

/// Callback type for registering a plugin-backed provider with the system.
pub type ProviderRegisterFn =
    Arc<dyn Fn(String, Arc<dyn crate::providers::Provider + Send + Sync>) + Send + Sync>;

/// Callback type for unregistering a plugin-backed provider.
pub type ProviderUnregisterFn = Arc<dyn Fn(String) + Send + Sync>;

/// Plugin manager - high-level interface for plugin operations
#[allow(dead_code)]
pub struct PluginManager {
    runtime: Arc<PluginRuntime>,
    hook_registry: Arc<HookRegistry>,
    plugins_dir: PathBuf,
    auto_load: bool,
    tool_registry: RwLock<Option<Arc<ToolRegistry>>>,
    trace_enabled: Arc<AtomicBool>,
    provider_register: RwLock<Option<ProviderRegisterFn>>,
    provider_unregister: RwLock<Option<ProviderUnregisterFn>>,
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
            tool_registry: RwLock::new(None),
            trace_enabled: Arc::new(AtomicBool::new(false)),
            provider_register: RwLock::new(None),
            provider_unregister: RwLock::new(None),
        })
    }

 /// Set callbacks for registering / unregistering plugin-backed providers.
    pub async fn set_provider_callbacks(
        &self,
        register: ProviderRegisterFn,
        unregister: ProviderUnregisterFn,
    ) {
        let mut reg = self.provider_register.write().await;
        *reg = Some(register);
        let mut unreg = self.provider_unregister.write().await;
        *unreg = Some(unregister);
    }

 /// Attach a `ToolRegistry` so that plugin tools are automatically
 /// registered on load / unregistered on unload.
    pub async fn set_tool_registry(&self, registry: Arc<ToolRegistry>) {
        let mut tr = self.tool_registry.write().await;
        *tr = Some(registry);
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
                    match self.load_plugin(&path).await {
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

 /// Load a plugin from a directory and register its tools and providers.
    pub async fn load_plugin(&self, path: &std::path::Path) -> crate::Result<String> {
        let plugin_id = self.runtime.load_plugin(path).await?;

        if let Some(plugin) = self.runtime.get_plugin(&plugin_id).await {
            self.register_plugin_tools(&plugin).await;
            self.register_plugin_providers(&plugin).await;
        }

        Ok(plugin_id)
    }

 /// Unload a plugin, unregistering its tools, providers, and hooks.
    pub async fn unload_plugin(&self, plugin_id: &str) -> crate::Result<bool> {
        self.deregister_plugin_tools(plugin_id).await;
        self.deregister_plugin_providers(plugin_id).await;
        self.hook_registry.unregister_plugin(plugin_id).await;
        self.runtime.unload_plugin(plugin_id).await
    }

 /// Reload a plugin with state preservation.
 ///
 /// Preserves `PluginState::memory`, re-reads the manifest from disk,
 /// and re-registers tools into the `ToolRegistry`.
    pub async fn reload_plugin(&self, plugin_id: &str) -> crate::Result<String> {
        info!("Reloading plugin '{}'...", plugin_id);

        self.deregister_plugin_tools(plugin_id).await;
        self.deregister_plugin_providers(plugin_id).await;
        self.hook_registry.unregister_plugin(plugin_id).await;

        let reloaded_id = self.runtime.reload_plugin(plugin_id).await?;

        if let Some(plugin) = self.runtime.get_plugin(&reloaded_id).await {
            self.register_plugin_tools(&plugin).await;
            self.register_plugin_providers(&plugin).await;
        }

        info!("Plugin '{}' reloaded successfully", reloaded_id);
        Ok(reloaded_id)
    }

 /// Enable or disable plugin trace logging.
    pub fn set_trace_enabled(&self, enabled: bool) {
        self.trace_enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

 /// Register a plugin's tools into the `ToolRegistry`.
    async fn register_plugin_tools(&self, plugin: &PluginInstance) {
        let tool_registry = self.tool_registry.read().await;
        if let Some(ref registry) = *tool_registry {
            for tool in plugin.manifest.get_tools() {
                let wrapper = Arc::new(PluginToolWrapper::new(
                    plugin.id().to_string(),
                    tool,
                    self.runtime.clone(),
                    self.trace_enabled.clone(),
                ));
                registry.register_dynamic(wrapper);
                info!("Registered plugin tool '{}' from plugin '{}'", tool.name, plugin.id());
            }
        }
    }

 /// Deregister a plugin's tools from the `ToolRegistry`.
    async fn deregister_plugin_tools(&self, plugin_id: &str) {
        let tool_registry = self.tool_registry.read().await;
        if let Some(ref registry) = *tool_registry {
            if let Some(plugin) = self.runtime.get_plugin(plugin_id).await {
                for tool in plugin.manifest.get_tools() {
                    registry.deregister_dynamic(&tool.name);
                    debug!("Deregistered plugin tool '{}' from plugin '{}'", tool.name, plugin_id);
                }
            }
        }
    }

 /// Register a plugin's provider capabilities with the system.
    async fn register_plugin_providers(&self, plugin: &PluginInstance) {
        let register_fn = self.provider_register.read().await;
        if let Some(ref register) = *register_fn {
            if let Some(ref capabilities) = plugin.manifest.capabilities {
                for cap in capabilities {
                    if let PluginCapability::Provider {
                        name,
                        default_model,
                        stream_family,
                        supports_tools,
                        max_context,
                    } = cap
                    {
                        let family = PluginProvider::parse_stream_family(stream_family);
                        let provider = Arc::new(PluginProvider::new(
                            plugin.id().to_string(),
                            name.clone(),
                            default_model.clone(),
                            *supports_tools,
                            *max_context,
                            family,
                            self.runtime.clone(),
                        ));
                        register(name.clone(), provider);
                        info!(
                            "Registered plugin provider '{}' from plugin '{}'",
                            name,
                            plugin.id()
                        );
                    }
                }
            }
        }
    }

 /// Deregister a plugin's providers from the system.
    async fn deregister_plugin_providers(&self, plugin_id: &str) {
        let unregister_fn = self.provider_unregister.read().await;
        if let Some(ref unregister) = *unregister_fn {
            if let Some(plugin) = self.runtime.get_plugin(plugin_id).await {
                if let Some(ref capabilities) = plugin.manifest.capabilities {
                    for cap in capabilities {
                        if let PluginCapability::Provider { name, .. } = cap {
                            unregister(name.clone());
                            debug!(
                                "Deregistered plugin provider '{}' from plugin '{}'",
                                name, plugin_id
                            );
                        }
                    }
                }
            }
        }
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
    pub async fn create_template(&self, name: &str, description: &str) -> crate::Result<PathBuf> {
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
        tokio::fs::write(plugin_dir.join("config.json"), serde_json::to_string_pretty(&config)?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn new_creates_empty_manager() {
        let tmp = tempdir().unwrap();
        let manager = PluginManager::new(tmp.path().to_path_buf()).await.unwrap();
        let plugins = manager.list_plugins().await;
        assert!(plugins.is_empty());
    }

    #[tokio::test]
    async fn create_template_creates_manifest() {
        let tmp = tempdir().unwrap();
        let manager = PluginManager::new(tmp.path().to_path_buf()).await.unwrap();

        let path = manager
            .create_template("test-plugin", "A test plugin")
            .await
            .unwrap();
        assert!(path.join("plugin.json").exists());
        assert!(path.join("config.json").exists());
        assert!(path.join("README.md").exists());
    }

    #[tokio::test]
    async fn initialize_loads_plugins_from_disk() {
        let tmp = tempdir().unwrap();
        let manager = PluginManager::new(tmp.path().to_path_buf()).await.unwrap();

 // Create two plugin templates
        manager
            .create_template("plugin-a", "Plugin A")
            .await
            .unwrap();
        manager
            .create_template("plugin-b", "Plugin B")
            .await
            .unwrap();

        let count = manager.initialize().await.unwrap();
        assert_eq!(count, 2);

        let plugins = manager.list_plugins().await;
        assert_eq!(plugins.len(), 2);
    }

    #[tokio::test]
    async fn load_and_unload_plugin() {
        let tmp = tempdir().unwrap();
        let manager = PluginManager::new(tmp.path().to_path_buf()).await.unwrap();

        let path = manager
            .create_template("load-test", "Load test")
            .await
            .unwrap();
        let id = manager.load_plugin(&path).await.unwrap();
        assert_eq!(id, "com.example.load-test");

        let plugins = manager.list_plugins().await;
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name(), "load-test");

        let unloaded = manager.unload_plugin(&id).await.unwrap();
        assert!(unloaded);

        let plugins = manager.list_plugins().await;
        assert!(plugins.is_empty());
    }

    #[tokio::test]
    async fn get_plugin_returns_some_when_exists() {
        let tmp = tempdir().unwrap();
        let manager = PluginManager::new(tmp.path().to_path_buf()).await.unwrap();

        let path = manager
            .create_template("get-test", "Get test")
            .await
            .unwrap();
        let id = manager.load_plugin(&path).await.unwrap();

        let plugin = manager.get_plugin(&id).await;
        assert!(plugin.is_some());
        assert_eq!(plugin.unwrap().name(), "get-test");

        let missing = manager.get_plugin("nonexistent").await;
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn set_enabled_toggles_plugin() {
        let tmp = tempdir().unwrap();
        let manager = PluginManager::new(tmp.path().to_path_buf()).await.unwrap();

        let path = manager
            .create_template("toggle-test", "Toggle test")
            .await
            .unwrap();
        let id = manager.load_plugin(&path).await.unwrap();

        let plugin = manager.get_plugin(&id).await.unwrap();
        assert!(plugin.enabled);

        manager.set_enabled(&id, false).await.unwrap();
        let plugin = manager.get_plugin(&id).await.unwrap();
        assert!(!plugin.enabled);

        manager.set_enabled(&id, true).await.unwrap();
        let plugin = manager.get_plugin(&id).await.unwrap();
        assert!(plugin.enabled);
    }

    #[tokio::test]
    async fn set_enabled_unknown_plugin_fails() {
        let tmp = tempdir().unwrap();
        let manager = PluginManager::new(tmp.path().to_path_buf()).await.unwrap();

        let result = manager.set_enabled("nonexistent", false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn reload_plugin_updates_manifest() {
        let tmp = tempdir().unwrap();
        let manager = PluginManager::new(tmp.path().to_path_buf()).await.unwrap();

        let path = manager
            .create_template("reload-test", "Reload test")
            .await
            .unwrap();
        let id = manager.load_plugin(&path).await.unwrap();

 // Modify manifest on disk
        let manifest_path = path.join("plugin.json");
        let mut manifest: PluginManifest = {
            let content = tokio::fs::read_to_string(&manifest_path).await.unwrap();
            serde_json::from_str(&content).unwrap()
        };
        manifest.description = "Updated description".to_string();
        tokio::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap())
            .await
            .unwrap();

        let reloaded_id = manager.reload_plugin(&id).await.unwrap();
        assert_eq!(reloaded_id, id);

        let plugin = manager.get_plugin(&id).await.unwrap();
        assert_eq!(plugin.manifest.description, "Updated description");
    }

    #[tokio::test]
    async fn reload_unknown_plugin_fails() {
        let tmp = tempdir().unwrap();
        let manager = PluginManager::new(tmp.path().to_path_buf()).await.unwrap();

        let result = manager.reload_plugin("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn hook_registry_is_empty_by_default() {
        let tmp = tempdir().unwrap();
        let manager = PluginManager::new(tmp.path().to_path_buf()).await.unwrap();

        let hooks = manager.hook_registry().list_hooks().await;
        assert!(hooks.is_empty());
    }

    #[tokio::test]
    async fn shutdown_clears_plugins() {
        let tmp = tempdir().unwrap();
        let manager = PluginManager::new(tmp.path().to_path_buf()).await.unwrap();

        let path = manager
            .create_template("shutdown-test", "Shutdown test")
            .await
            .unwrap();
        manager.load_plugin(&path).await.unwrap();

        manager.shutdown().await.unwrap();
        let plugins = manager.list_plugins().await;
        assert!(plugins.is_empty());
    }

    #[tokio::test]
    async fn unload_unknown_plugin_returns_false() {
        let tmp = tempdir().unwrap();
        let manager = PluginManager::new(tmp.path().to_path_buf()).await.unwrap();

        let result = manager.unload_plugin("nonexistent").await.unwrap();
        assert!(!result);
    }
}

/// Plugin tool wrapper - adapts plugin tools to Syscity's Tool trait
use crate::tools::{Tool, ToolContext, ToolExecutionResult};

pub struct PluginToolWrapper {
    plugin_id: String,
    tool_name: String,
    description: String,
    parameters: serde_json::Value,
    runtime: Arc<PluginRuntime>,
    trace_enabled: Arc<std::sync::atomic::AtomicBool>,
}

impl PluginToolWrapper {
    pub fn new(
        plugin_id: String,
        tool: &PluginTool,
        runtime: Arc<PluginRuntime>,
        trace_enabled: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            plugin_id,
            tool_name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
            runtime,
            trace_enabled,
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

        if self
            .trace_enabled
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            debug!(
                "[trace] Plugin tool '{}' from '{}' called with args: {}",
                self.tool_name, self.plugin_id, args
            );
        }

        let result = self
            .runtime
            .call_tool(&self.plugin_id, &self.tool_name, args)
            .await;

        if self
            .trace_enabled
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            debug!(
                "[trace] Plugin tool '{}' from '{}' result: {:?} (elapsed: {:?})",
                self.tool_name,
                self.plugin_id,
                result.is_ok(),
                start.elapsed()
            );
        }

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
