# Plugins Module

WASM-based sandboxed plugin system for runtime extensibility.

## Design

Plugins extend Manta with custom tools, hooks, and channels without modifying core code. Each plugin is a directory with a `plugin.json` manifest.

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

`PluginToolWrapper` bridges between the Manta `Tool` trait and the plugin's WASM-exported functions, with optional trace logging.

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

## Missing / TODO

- **Missing**: WASM sandbox may not fully isolate memory/IO — `PluginRuntime` needs WASI capability restrictions (filesystem, network, env) via `wasmtime_wasi::WasiCtxBuilder`.
- **Missing**: Plugin channels capability is declared but not wired into `ChannelRegistry`.
- **Missing**: Plugin-to-plugin communication.
- **Missing**: Plugin marketplace / registry — remote install, plugin index format, semver dependency resolution.
- **Missing**: Plugin signing and verification (ed25519-dalek or similar).
- **Missing**: Granular permission enforcement at runtime (manifest declares permissions but enforcement is coarse).
- **Missing**: Plugin state persistence across restarts.
- **Missing**: Plugin metrics and resource usage monitoring.
- **Missing**: File-system watcher hot reload — `reload_plugin()` exists but no `notify` watcher for `.wasm` file changes.
- **Missing**: Plugin registry with SQLite persistence for installed plugin metadata.
- **Missing**: Activation planner — trigger-based plugin loading (command/provider/channel/route/capability), dependency-ordered activation, diagnostics.
- **Missing**: Version management — semver compatibility checking (`manta = ">=0.1.0, <0.2.0"`), plugin version sync, multi-version coexistence via wasmtime module isolation.
- **Missing**: Config hot-reload integration — `ConfigWatcher` changes should diff plugin list, safely load/unload changed plugins.
- **Missing**: Plugin dependency management — auto-download external resources (binaries, models), `dirs`-based data directory.
- **Missing**: Migration system — plugin data structure changes with SQLite `schema_version` tracking.
- **Missing**: Modular SDK crates — workspace-based `manta-plugin-sdk-core`, `manta-plugin-sdk-channel`, `manta-plugin-sdk-memory`, `manta-plugin-sdk-provider`, `manta-plugin-sdk-security` to enforce boundary control.
- **Missing**: SDK boundary lint — CI check that plugins only depend on SDK crates, not internal `src/` modules.
- **Missing**: Plugin doctor — compatibility and runtime environment diagnostics at load time.
- **Missing**: WIT interface versioning for WASM plugins.
