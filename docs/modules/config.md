# Config Module

Configuration management, hot reload system, and runtime reconfiguration without restart.

## Architecture

Configuration is split into two layers:

- **`Config`** (`src/config.rs`) — The main application configuration, deserialized from `syscity.toml`. Contains all subsystems as fields.
- **`HotReloadManager`** (`src/config.rs:hot_reload`) — File watcher + event dispatch that detects config changes and triggers per-type handlers.

### Config Fields

| Category | Fields | Reload Support |
|----------|--------|----------------|
| Server | `host`, `port`, `timeout_seconds`, `max_body_size` | Restart required |
| Logging | `level`, `format`, `file`, `stdout`, `rotation` | Hot-reloadable |
| Storage | `storage_type`, `connection`, `database` | Restart required |
| Services | `services` (HashMap of external service configs) | Hot-reloadable |
| Browser | `browser` (bridge, pool, profiles) | Hot-reloadable |
| Memory | `memory` (multimodal, dreaming, tier, effectiveness) | Hot-reloadable |
| Heartbeat | `heartbeat` | Hot-reloadable |
| Computer | `computer` (remote_control, headless) | Hot-reloadable |
| Standing Orders | `standing_orders` | Hot-reloadable |
| Capabilities | `capabilities` (profile, scope, sets) | Hot-reloadable |

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

### Config Loading Order

1. Default values
2. Config file (`syscity.toml` or specified path)
3. Environment variables (`SYSCITY_*`)

### Secret Resolution

`Config::resolve_secrets()` uses `SecretResolver` to resolve `SecretRef` values in service configurations via environment variables, files, or external executables.

## Key Types

```rust
pub struct Config {
    pub schema_version: u32,
    pub app: AppConfig,
    pub server: ServerConfig,
    pub logging: LoggingConfig,
    pub storage: StorageConfig,
    pub services: HashMap<String, ServiceConfig>,
    pub browser: BrowserConfig,
    pub memory: MemoryConfig,
    pub heartbeat: HeartbeatConfig,
    pub computer: ComputerConfig,
    pub standing_orders: StandingOrderConfig,
    pub capabilities: CapabilitiesConfig,
    pub extra: HashMap<String, toml::Value>,
}
```

```rust
pub struct HotReloadManager {
    watched_files: Arc<RwLock<HashMap<PathBuf, WatchedConfig>>>,
    handlers: Arc<RwLock<HashMap<ConfigFileType, Vec<ConfigChangeHandler>>>>,
    change_tx: mpsc::Sender<ConfigChangeEvent>,
    change_rx: Arc<RwLock<mpsc::Receiver<ConfigChangeEvent>>>,
    #[cfg(feature = "hot-reload")]
    watcher: Arc<RwLock<Option<Debouncer<RecommendedWatcher, FileIdMap>>>>,
}
```

```rust
pub struct ConfigChangeEvent {
    pub path: PathBuf,
    pub config_type: ConfigFileType,
    pub change_type: ConfigChangeType,
}
```

## Implemented Features

- Multi-source configuration loading (defaults, TOML file, environment variables)
- Environment variable interpolation in TOML (`$VAR` and `${VAR}`)
- Schema versioning with migration support
- Cross-field validation (ports, log levels, protocols, storage connections)
- File watcher with 500ms debounce for hot reload
- Per-config-type handler registration
- Secret resolution with `SecretRef` (env, file, exec providers)
- `ReloadableConfig` with broadcast-based change notifications
- Comprehensive unit tests for all config subsystems

