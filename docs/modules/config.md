# Config Module

Configuration management, hot reload system, and the `HotReloadManager` for runtime reconfiguration without restart.

## Architecture

Configuration is split into two layers:

- **`GatewayConfig`** (`src/gateway/mod.rs:68-144`) — The main runtime configuration, deserialized from `syscity.toml`. Contains all subsystems as fields.
- **`HotReloadManager`** (`src/config.rs:1252-1611`) — File watcher + event dispatch that detects config changes and triggers per-type handlers.

### GatewayConfig Fields

| Category | Fields | Reload Support |
|----------|--------|----------------|
| Network | `host`, `port`, `tailscale_enabled`, `tailscale_domain` | Restart required |
| Agent | `default_agent` | Restart, or per-agent via `ConfigFileType::Agent` |
| Channels | `channels` (`HashMap<String, ChannelConfig>`) | Hot-reloadable (add/remove/restart) |
| Storage | `storage`, `vector_memory` | Restart required |
| Plugins | `plugins` | Hot-reloadable (file watcher + CLI) |
| Providers | `providers` (`HashMap<String, ProviderConfig>`) | Hot-reloadable (synced to ModelRouter) |
| MCP | `mcp` | Hot-reloadable (disconnect/reconnect servers) |
| Security | `security` (auth, rate limit, CORS) | Hot-reloadable (subset) |
| Model | `model`, `model_provider` | Hot-reloadable |
| Computer | `computer` (desktop automation) | Hot-reloadable |
| Browser | `browser` | Hot-reloadable |
| Scheduling | `cron`, `heartbeat` | Hot-reloadable |
| Cost | `cost_guard` | Hot-reloadable |
| Agent runtime | `capabilities`, `standing_orders` | Hot-reloadable |
| Hot reload | `hot_reload` | Hot-reloadable (self-referential) |
| Memory | `dreaming` | Hot-reloadable |
| Workspace | `workspace_dir`, `workspace_only` | Hot-reloadable |

### Hot Reload Architecture

```
syscity.toml ──▶ notify debouncer ──▶ mpsc::channel ──▶ HotReloadManager::run()
                  (500ms delay)                            │
                                                          ├── ConfigFileType::Main ──▶ channels add/remove/restart
                                                          ├── ConfigFileType::Agent ──▶ send UpdateConfig to agent
                                                          ├── ConfigFileType::Channel ──▶ reinit single channel
                                                          ├── ConfigFileType::Plugin ──▶ reload_plugin()
                                                          └── ConfigFileType::Gateway ──▶ security/providers/mcp/hot_reload
```

### ConfigFileType

```rust
pub enum ConfigFileType {
    Main,     // syscity.toml — channel lifecycle
    Agent,    // agents/*.toml — per-agent config
    Channel,  // standalone channel config files
    Plugin,   // WASM files or plugin manifests
    Gateway,  // gateway subsection (security, providers, mcp, hot_reload)
    Custom,   // user-defined config files
}
```

### CLI Reload

`syscity reload` triggers `POST /api/v1/reload` which performs a comprehensive reload:

| Scope | What reloads |
|-------|-------------|
| `plugins` | Unload all plugins, reinitialize from disk |
| `config` | Read `syscity.toml`, update hot-reloadable fields |
| `providers` | Sync providers with ModelRouter (add new, remove deleted) |
| `mcp` | Disconnect all MCP servers, reconnect from new config |
| `skills` | Reinitialize SkillManager |
| `all` (default) | All of the above |

Channels are hot-reloaded automatically by the file watcher when `syscity.toml` changes.

## Key Types

```rust
pub struct HotReloadManager {
    watched_files: Arc<RwLock<HashMap<PathBuf, WatchedConfig>>>,
    handlers: Arc<RwLock<HashMap<ConfigFileType, Vec<ConfigChangeHandler>>>>,
    change_tx: mpsc::Sender<ConfigChangeEvent>,
    change_rx: Arc<RwLock<mpsc::Receiver<ConfigChangeEvent>>>,
    // Optional notify-based file watcher (feature-gated)
    watcher: Arc<RwLock<Option<Debouncer<RecommendedWatcher, FileIdMap>>>>,
}

pub struct ConfigChangeEvent {
    pub path: PathBuf,
    pub config_type: ConfigFileType,
    pub change_type: ConfigChangeType,
}
```

## Missing / TODO

- **📝 Partial**: Config schema validation beyond serde — `GatewayConfig` uses serde `#[serde(deny_unknown_fields)]` for catch-all validation. Missing: JSON Schema generation, cross-field validation.
- **❌ Missing**: Config migration system for version upgrades (schema version tracking).
- **❌ Missing**: Environment variable interpolation inside config files (e.g., `url = "${API_URL}"`).
