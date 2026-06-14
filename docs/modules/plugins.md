# Plugins Module

WASM-based sandboxed plugin system for runtime extensibility.

## Design

Plugins extend Syscity with custom tools, hooks, channels, and providers without modifying core code. Each plugin is a directory with a `plugin.json` manifest.

- **`PluginManager`** — High-level interface: load, unload, reload, list plugins; auto-load on startup
- **`PluginRuntime`** — Low-level WASM runtime (wasmer-based sandbox)
- **`PluginManifest`** — Declares plugin ID, name, version, capabilities, permissions, tools, hooks, commands
- **`HookRegistry`** / **`HookHandler`** — Before/after/policy hooks for tool execution and message processing
- **`PluginInstance`** — Runtime representation of a loaded plugin
- **`ActivationPlanner`** — Lazy loading and dependency ordering for plugin activation
- **`PluginSqliteRegistry`** — Persistent SQLite-backed plugin metadata store
- **`PluginMetricsRegistry`** — Metrics collection for plugin performance
- **`RegistryClient`** / **`RegistryIndex`** — Remote plugin registry client for discovery and installation
- **`PluginInstaller`** — Plugin installation from remote registries
- **`DependencyResolver`** — Resolves plugin dependency chains
- **`Verification`** — Manifest verification and security checks

### Capabilities

Plugins declare capabilities in their manifest:
- `Tools` — Register custom tools in `ToolRegistry`
- `Hooks` — Subscribe to execution hooks
- `Channels` — Register custom communication channels
- `Memory` — Access to memory store
- `Providers` — Register custom LLM providers

### Permissions

- `Memory` — Read/write persistent memory
- `FileSystem` — File system access
- `Network` — Outbound network requests
- `Shell` — Shell command execution

### Lifecycle

```
Plugin Directory ──▶ PluginManifest ──▶ PluginRuntime::load()
                                              │
                                              ├──▶ register_plugin_tools()
                                              │      └── ToolRegistry::register_dynamic()
                                              ├──▶ register_hooks()
                                              │      └── HookRegistry::register()
                                              ├──▶ register_plugin_providers()
                                              │      └── ProviderRegisterFn callback
                                              └──▶ PluginInstance (active)
```

### Hot Reload

`reload_plugin()` preserves plugin memory state, re-reads the manifest, and re-registers tools/hooks.

### Tool Wrapper

`PluginToolWrapper` bridges between the Syscity `Tool` trait and the plugin's WASM-exported functions, with optional trace logging.

### Provider Extension

`PluginProvider` + `PluginProviderRegistry` allow WASM-backed LLM providers to be registered dynamically. Callbacks (`ProviderRegisterFn`, `ProviderUnregisterFn`) are set on `PluginManager` to wire into the system.

## Key Types

```rust
pub struct PluginManager {
    runtime: Arc<PluginRuntime>,
    hook_registry: Arc<HookRegistry>,
    plugins_dir: PathBuf,
    auto_load: bool,
    tool_registry: RwLock<Option<Arc<ToolRegistry>>>,
    trace_enabled: Arc<AtomicBool>,
    provider_register: RwLock<Option<ProviderRegisterFn>>,
    provider_unregister: RwLock<Option<ProviderUnregisterFn>>,
    channel_register: RwLock<Option<ChannelRegisterFn>>,
    channel_unregister: RwLock<Option<ChannelUnregisterFn>>,
    sqlite_registry: RwLock<Option<PluginSqliteRegistry>>,
    activation_planner: RwLock<Option<ActivationPlanner>>,
}

pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub capabilities: Option<Vec<PluginCapability>>,
    pub permissions: Option<Vec<PluginPermission>>,
    pub config: Option<serde_json::Value>,
}

pub enum PluginCapability {
    Tools { tools: Vec<PluginTool> },
    Hooks { hooks: Vec<String> },
    Channels,
    Providers,
}

pub enum HookType {
    BeforeToolExecute,
    AfterToolExecute,
    BeforeMessageProcess,
    AfterMessageProcess,
}
```

## Implemented Features

- WASM-based sandboxed plugin runtime
- Auto-load plugins from directory on startup
- Dynamic tool registration via `ToolRegistry`
- Hook system for before/after tool execution and message processing
- Channel plugin support via `ExtendedChannelRegistry`
- Provider plugin support for custom LLM backends
- Plugin manifest verification and security checks
- SQLite-backed persistent plugin registry
- Activation planner for lazy loading and dependency ordering
- Remote registry client for plugin discovery and installation
- Plugin metrics collection and reporting
- Hot reload with state preservation
- Trace logging for plugin tool execution
- Sync filesystem plugins into SQLite registry

