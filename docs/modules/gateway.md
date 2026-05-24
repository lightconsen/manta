# Gateway Module

The control plane for Manta, managing channels, agents, and the HTTP/WebSocket API.

## Design

- **`Gateway`** — Main struct that owns:
  - `GatewayState` — shared state (memory manager, channel registry, agent pool, tool registry, plugin manager)
  - Axum router for HTTP API + WebSocket
  - Channel lifecycle management
- **`GatewayState`** — `Arc`-shared state with `RwLock` fields for dynamic components
- **Auth** (`auth.rs`) — JWT-based authentication, API key validation
- **Rate Limiting** (`rate_limit.rs`) — Token bucket rate limiter per client
- **Webhooks** (`webhooks.rs`) — Incoming webhook handlers
- **Middleware** (`middleware.rs`) — CORS, auth, logging
- **Protocol** (`protocol.rs`) — ACP protocol handlers
- **Commands** (`commands.rs`) — Gateway control commands

### Startup Flow

1. Load configuration
2. Initialize `MemoryManager` (tiered or unified based on config)
3. Initialize `ToolRegistry` with built-in + MCP tools
4. Initialize `ChannelRegistry` with configured channels
5. Start channels (`init_channels()`)
6. Start `DreamScheduler` (if tiered memory is enabled)
7. Start HTTP server (Axum)

### WebSocket

`ws.rs` handles real-time client connections:
- Bidirectional message streaming
- Progress event forwarding
- Agent output streaming

## Key Types

```rust
pub struct Gateway {
    state: Arc<GatewayState>,
    config: GatewayConfig,
    listener: Option<TcpListener>,
}

pub struct GatewayState {
    pub memory_manager: RwLock<Option<MemoryManager>>,
    pub channel_registry: RwLock<ChannelRegistry>,
    pub tool_registry: Arc<ToolRegistry>,
    pub plugin_manager: RwLock<PluginManager>,
    // ...
}
```

## Missing / TODO

- **Missing**: Management REST handlers are transitional (kept in source but not routed per protocol.md v1.0 Phase 3).
- **Missing**: Full ACP control plane integration — ACP struct exists but protocol handlers are incomplete.
- **Missing**: Gateway-level metrics and health check endpoint.
- **Missing**: Multi-instance coordination (distributed mode).
- **Missing**: Send policy enforcement (DM policy, allowlist gate) not fully wired.
- **Missing**: Gateway Protocol Schema — JSON Schema / OpenAPI spec generation from Rust types (e.g. via `schemars`), multi-language binding generation pipeline.
- **Missing**: Device identity system — `DeviceIdentity` with device ID, platform, version, persistent storage in SQLite, device pairing (QR code).
- **Missing**: Multi-auth mode gateway — `token` / `password` / `none` with mode ambiguity detection, credential precedence (`env-first` / `config-first`), Tailscale and trusted proxy auth.
- **Missing**: TUI client — `ratatui`-based interactive terminal UI with real-time streaming, session management, config editor.
- **Missing**: Web UI static file serving and WebSocket real-time integration (`web/chat-ui/` exists but may not be fully wired to Gateway WebSocket).
- **Missing**: Multi-platform native clients — mobile (Tauri Mobile / React Native + Gateway), desktop (Tauri / egui / iced), shared protocol layer (`uniffi` or JSON Schema → Swift/Kotlin/TypeScript).
- **Missing**: Protocol code generation pipeline — automated Rust Types → JSON Schema → Swift/Kotlin/TypeScript bindings, CI compatibility verification.
