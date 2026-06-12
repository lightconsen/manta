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

## Missing / TODO

- **Missing**: WASM sandbox may not fully isolate memory/IO — `PluginRuntime` needs WASI capability restrictions (filesystem, network, env) via `wasmtime_wasi::WasiCtxBuilder`.
- **✅ Implemented**: Plugin channels capability — `PluginChannel` implements `Channel` trait (`src/channels/plugin_host.rs:637-794`), `PluginChannelRegistry` manages lifecycle (`src/channels/plugin_host.rs:807-997`), `ExtendedChannelRegistry` provides unified native+plugin access (`src/channels/mod.rs:957-1110`), gateway wires them at startup (`src/gateway/mod.rs:3173-3195`). `PluginManager` now participates in channel management: `register_plugin_channels()` / `deregister_plugin_channels()` methods handle `PluginCapability::Channel` from manifests, wired via callbacks to GatewayState's channel map (`src/plugins/mod.rs`, `src/gateway/mod.rs`).
- **✅ Implemented**: Plugin-to-plugin communication — `emit_event` host function and `PluginEvent` type exist, plugins can emit events. Event bus wired: subscription/dispatch mechanism in `PluginRuntime` (`subscribe_events`, `unsubscribe_events`, background dispatch task via `event_dispatch_loop`). See `src/plugins/runtime.rs:188-226`.
- **Missing**: Plugin marketplace / registry — remote install, plugin index format, semver dependency resolution.
- **Missing**: Plugin signing and verification (ed25519-dalek or similar).
- **Missing**: Granular permission enforcement at runtime — `PluginPermission` enum is fully declared (`src/plugins/manifest.rs:154-170`) but not enforced by `PluginRuntime` host functions.
- **✅ Implemented**: Plugin state persistence across restarts — `PluginPersistentState` with `save_plugin_state()` and `load_plugin_state()` methods serialize plugin memory + KV store to `~/.syscity/plugins/data/{id}/state.json`. Integrated into `load_plugin()`, `unload_plugin()`, and `shutdown()`. See `src/plugins/runtime.rs:813-873`, `src/dirs.rs:168-171`.
- **Missing**: Plugin metrics and resource usage monitoring.
- **✅ Implemented**: File-system watcher hot reload — WASM files are watched via `HotReloadManager::watch_file()` at startup, and a `ConfigFileType::Plugin` handler reloads plugins on change (state-preserving reload with unload+load fallback). See `src/gateway/mod.rs:2042-2067` and `src/gateway/mod.rs:4381-4458`.
- **Missing**: Plugin registry with SQLite persistence for installed plugin metadata.
- **Missing**: Activation planner — trigger-based plugin loading (command/provider/channel/route/capability), dependency-ordered activation, diagnostics.
- **✅ Implemented**: Version management — `PluginManifest.version` is validated via `crate::skills::semver::Version::parse()` at load/reload time. `syscity_version` constraint field added to manifest. `validate_manifest_version()` checks both fields (non-fatal warnings). See `src/plugins/manifest.rs:15-18`, `src/plugins/runtime.rs:875-912`.
- **✅ Implemented**: Config hot-reload integration — `ConfigFileType::Plugin` handler responds to WASM/manifest changes with state-preserving reload; `syscity reload` CLI triggers comprehensive reload of plugins + config + providers + MCP + skills via `POST /api/v1/reload`. See `src/gateway/handlers/admin.rs:483-706`.
- **Missing**: Plugin dependency management — auto-download external resources (binaries, models), `dirs`-based data directory.
- **Missing**: Migration system — plugin data structure changes with SQLite `schema_version` tracking.
- **✅ Implemented**: Modular SDK crates — `crates/syscity-plugin-sdk` (general plugins) and `crates/syscity-channel-sdk` (channel plugins) as workspace members (`Cargo.toml:11-12`), each with published WIT interfaces.
- **Missing**: SDK boundary lint — CI check that plugins only depend on SDK crates, not internal `src/` modules.
- **✅ Implemented**: Plugin diagnostic tools — `PluginManager::diagnose()` checks plugin semver, syscity_version, WASM file existence/compilation, capability consistency. Doctor `run_diagnostics()` queries daemon `/api/v1/plugins` endpoint for active plugin diagnostics. See `src/plugins/mod.rs:263-350`, `src/cli/doctor.rs:471-499`.
- **✅ Implemented**: WIT interface versioning for WASM plugins — `syscity:plugin-sdk@0.2.0` (`wit/plugin-sdk/plugin-sdk.wit`) and `syscity:channel@0.1.0` (`wit/channel.wit`), both with defined import/export worlds.
