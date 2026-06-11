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

Config changes are now tracked with snapshot-based diff:
- `GatewayConfig::snapshot()` captures hot-reloadable fields before reload
- `GatewayConfig::diff_since()` compares current state against a snapshot
- Results are logged to `PersistentAuditLog` (`AuditEventType::ConfigChange`)
- Wired into both `POST /api/v1/reload` (CLI) and `HotReloadManager Gateway` handler (file watcher)

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

```rust
/// Snapshot of hot-reloadable fields for change detection.
pub struct ConfigSnapshot {
    pub timestamp: String,
    pub fields: HashMap<String, serde_json::Value>,
}

/// A single field change detected during hot reload.
pub struct ConfigChange {
    pub path: String,
    pub old_value: Option<serde_json::Value>,
    pub new_value: Option<serde_json::Value>,
}
```

## Missing / TODO

- **✅ Implemented**: Config schema version migration — `schema_version` field, `CURRENT_SCHEMA_VERSION` constant, and `migrate()` with sequential v0→v1 support. Auto-applied on load when `config.schema_version < CURRENT_SCHEMA_VERSION`. See `src/config.rs:19-24`, `src/config.rs:968-986`.
- **✅ Implemented**: Config schema validation — `Config::validate()` checks individual fields (port, log level, ...) and cross-field constraints (e.g., `storage_type: database` requires `connection`; heartbeat time format validation). A JSON Schema endpoint is available at `GET /api/v1/config/schema` (`config_schema_handler` in `src/gateway/handlers/health.rs:277-359`, removed in a later cleanup). Neither `Config` nor `GatewayConfig` use `#[serde(deny_unknown_fields)]`. See `src/config.rs:1026-1070`.
- **✅ Implemented**: Environment variable interpolation — `Config::interpolate_env_vars()` pre-processes raw TOML with regex before `toml::from_str()`, supporting `${VAR}`, `$VAR`, and `$$VAR` (escape → literal `$VAR`). Applied in `load_from_file()`. See `src/config.rs:976-1007`.
- **✅ Implemented**: Config diff / audit trail — `GatewayConfig::snapshot()` captures hot-reloadable fields to a `ConfigSnapshot`; `diff_since()` computes `Vec<ConfigChange>` with path/old/new values. Wired into `reload_all_handler` (`src/gateway/handlers/admin.rs`) and `HotReloadManager Gateway` handler (`src/gateway/mod.rs:4459-4493`). Changes are logged to `PersistentAuditLog` with `AuditEventType::ConfigChange` and emitted via `tracing::info!`.
