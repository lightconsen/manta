# Config Module

Configuration management with hot reload support.

## Design

- **`Config`** — Main configuration struct with serde deserialization from YAML/TOML
- **`ConfigWatcher`** — File system watcher for configuration changes
- **`HotReloadManager`** — Manages reloadable config sections with event broadcasting
- **Config change events** — `ConfigChangeEvent` with `ConfigChangeType` (create, modify, delete, rename)

### Hot Reload Architecture

```
Config File ──▶ File Watcher ──▶ HotReloadManager ──▶ ConfigChangeEvent
                                                         │
                                              ┌─────────┼─────────┐
                                              ▼         ▼         ▼
                                          Channels   Memory    Providers
```

### Supported Config Files

- `config.yaml` / `config.toml` — Main configuration
- `syscity.yaml` — Workspace-level overrides
- Feature-specific configs (channels, providers, memory)

## Key Types

```rust
pub struct Config {
    pub server: ServerConfig,
    pub channels: HashMap<String, ChannelConfig>,
    pub providers: HashMap<String, ProviderConfig>,
    pub memory: MemoryConfig,
    pub tools: ToolConfig,
    pub security: SecurityConfig,
}

pub struct HotReloadManager {
    watched_configs: Vec<WatchedConfig>,
    change_tx: broadcast::Sender<ConfigChangeEvent>,
}
```

## Missing / TODO

- **📝 Partial**: Config schema validation beyond serde — `Config::validate()` performs manual checks (port != 0, log level enum, etc.). See `src/config.rs:724-755`. Missing: JSON Schema validation, cross-field validation.
- **Missing**: Config migration system for version upgrades.
- **Missing**: Encrypted secrets in config (currently uses separate secrets module).
- **📝 Partial**: Environment variable interpolation — `load_from_env()` overrides specific fields via `SYSCITY_*` environment variables (`src/config.rs:614-721`). Missing: general interpolation syntax inside config files (e.g., `host = "${HOST}"`).
