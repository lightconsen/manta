# Gateway Module

The control plane for Syscity, managing channels, agents, and the HTTP/WebSocket API.

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

- **✅ Implemented**: Gateway health check endpoints — `/health`, `/ready`, `/live`, `/api/v1/health`, and `/api/v1/metrics` are routed and fully implemented. `/health` and `/api/v1/health` return a comprehensive `HealthReport` with subsystem statuses. `/api/v1/metrics` returns Prometheus text format with uptime, agent/channel/provider counts, memory readiness, cost guard, and audit log metrics. See `src/gateway/mod.rs:2035-2037` (routing) and `src/gateway/mod.rs:4356-4530` (handlers).
- **✅ Implemented**: Send policy enforcement — DM policy (Open/Pairing/Allowlist) and mention gating are evaluated in the inbound dispatch pipeline. See `src/inbound/dispatch.rs:91-103` and `src/gateway/mod.rs:690-769`.
- **📝 Partial**: Management REST handlers — handlers exist in `src/gateway/mod.rs` but are not fully routed per protocol.md v1.0 Phase 3.
- **📝 Partial**: Full ACP control plane integration — `ExecutionController` with pause/resume/step/cancel is wired into the agent tool loop (`src/acp/mod.rs:1833-1845`), but protocol handlers are transitional.
- **📝 Partial**: Device identity system — `DevicePairingStore` exists in `GatewayState` (`src/gateway/mod.rs:598,1418-1419`) with 5-character codes and 1h TTL. Missing: QR-code pairing, 8-character codes, max pending limit.
- **📝 Partial**: Multi-auth mode gateway — `AuthMode` enum exists (`src/gateway/mod.rs:316`) with None/Token/Device/Tailscale variants. Missing: mode ambiguity detection at startup, Tailscale `whois` verification, trusted proxy auth.
- **✅ Implemented**: Web UI — `web/src/` contains a full React/TypeScript SPA with WebSocket-based real-time streaming. The gateway serves it via Axum `frontend_router` at `/`, `/favicon.svg`, `/syscity.png`, and `/assets/*path`. See `src/gateway/mod.rs:2506-2511`.
- **❌ Missing**: Multi-instance coordination (distributed mode).
- **❌ Missing**: Gateway Protocol Schema — JSON Schema / OpenAPI spec generation from Rust types (e.g. via `schemars`), multi-language binding generation pipeline.
- **❌ Missing**: TUI client — `ratatui`-based interactive terminal UI with real-time streaming, session management, config editor.
- **❌ Missing**: Multi-platform native clients — mobile (Tauri Mobile / React Native + Gateway), desktop (Tauri / egui / iced), shared protocol layer (`uniffi` or JSON Schema → Swift/Kotlin/TypeScript).
- **❌ Missing**: Protocol code generation pipeline — automated Rust Types → JSON Schema → Swift/Kotlin/TypeScript bindings, CI compatibility verification.
