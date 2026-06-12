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
- **✅ Removed**: Management REST handlers — Entity CRUD and Team management handlers existed in `src/gateway/handlers/admin.rs` with `#[allow(dead_code)]`. They were kept for reference during protocol.md v1.0 Phase 3 (transitional) and have been removed in Phase 5 cleanup. The `reload_all_handler` and channel enable/disable handlers remain wired and active.
- **✅ Removed (covered by WebSocket)**: Full ACP control plane integration — `ExecutionController` with pause/resume/step/cancel is wired into the agent tool loop (`src/acp/mod.rs:1833-1845`). REST protocol handlers (`src/gateway/handlers/acp.rs`) were dead code (`#[allow(dead_code)]`, not routed) and have been removed — all ACP operations are covered by WebSocket handlers in `src/gateway/ws.rs`.
- **✅ Implemented**: Device identity system — `DevicePairingStore` (`src/security/device_pairing.rs`) with 8-character unambiguous codes, 1h TTL, configurable max pending limit (default 100), QR SVG generation via `qrcode` crate, and REST API endpoints (`/api/v1/device/pairing/*`) for listing pending/authorized, approve/reject/revoke, and QR code retrieval.
- **✅ Implemented**: Multi-auth mode gateway — `AuthMode` enum (`src/gateway/mod.rs:316`) with None/Token/Device/Tailscale variants. Tailscale whois verification via `TailscaleAuthenticator` (`src/security/tailscale.rs`) with caching. Trusted proxy support in `extract_client_ip_with_trusted()` (`src/gateway/middleware.rs`) — `X-Forwarded-For` only accepted from configured `trusted_proxies` or localhost. `ConnectInfo<SocketAddr>` extension via `into_make_service_with_connect_info`. `allowed_tailnets` config for tailnet-scoped access control.
- **✅ Implemented**: Web UI — `web/src/` contains a full React/TypeScript SPA with WebSocket-based real-time streaming. The gateway serves it via Axum `frontend_router` at `/`, `/favicon.svg`, `/syscity.png`, and `/assets/*path`. See `src/gateway/mod.rs:2506-2511`.
- **⏭️ Skipped**: Multi-instance coordination (distributed mode). Single-instance sufficient for current deployment scale.
- **⏭️ Skipped**: Gateway Protocol Schema — JSON Schema / OpenAPI spec generation from Rust types. No external third-party API consumers; Rust CLI communicates directly. Not needed currently.
- **❌ Missing**: TUI client — `ratatui`-based interactive terminal UI with real-time streaming, session management, config editor.
- **✅ Implemented**: Desktop client — Tauri 2 app in `desktop/` with tray icon. Serves the Web UI. Mobile clients (Tauri Mobile / React Native) not implemented.
- **⏭️ Skipped**: Protocol code generation pipeline — automated Rust Types → JSON Schema → Swift/Kotlin/TypeScript bindings. Superseded by skipped Gateway Protocol Schema. No mobile clients to consume generated bindings.
