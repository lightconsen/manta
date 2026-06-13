# Plugins Module

WASM-based sandboxed plugin system for runtime extensibility.

## Design

Plugins extend Syscity with custom tools, hooks, and channels without modifying core code. Each plugin is a directory with a `plugin.json` manifest.

- **`PluginManager`** — High-level interface: load, unload, reload, list plugins; auto-load on startup
- **`PluginRuntime`** — Low-level WASM runtime (wasmer-based sandbox)
- **`PluginManifest`** — Declares plugin ID, name, version, capabilities, permissions, tools, hooks, commands
- **`HookRegistry`** / **`HookHandler`** — Before/after/policy hooks for tool execution and message processing
- **`PluginInstance`** — Runtime representation of a loaded plugin

### Capabilities

Plugins declare capabilities in their manifest:
- `Tools` — Register custom tools in `ToolRegistry`
- `Hooks` — Subscribe to execution hooks
- `Channels` — Register custom communication channels
- `Memory` — Access to memory store

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
                                              └──▶ PluginInstance (active)
```

### Hot Reload

`reload_plugin()` preserves plugin memory state, re-reads the manifest, and re-registers tools/hooks.

### Tool Wrapper

`PluginToolWrapper` bridges between the Syscity `Tool` trait and the plugin's WASM-exported functions, with optional trace logging.

## Key Types

```rust
pub struct PluginManager {
    runtime: Arc<PluginRuntime>,
    hook_registry: Arc<HookRegistry>,
    plugins_dir: PathBuf,
    tool_registry: RwLock<Option<Arc<ToolRegistry>>>,
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
}

pub enum HookType {
    BeforeToolExecute,
    AfterToolExecute,
    BeforeMessageProcess,
    AfterMessageProcess,
}
```

