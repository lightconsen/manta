# Gateway Module

The control plane for Syscity, managing channels, agents, and the HTTP/WebSocket API.

## Design

- **`Gateway`** — Main struct that owns:
  - `GatewayState` — shared state (memory manager, channel registry, agent pool, tool registry, plugin manager)
  - Axum router for HTTP API + WebSocket
  - Channel lifecycle management
- **`GatewayState`** — `Arc`-shared state with `RwLock` fields for dynamic components
- **`GatewayConfig`** — Comprehensive runtime configuration with all subsystem fields
- **Auth** (`auth.rs`) — JWT-based authentication, API key validation
- **Rate Limiting** (`rate_limit.rs`) — Token bucket rate limiter per client
- **Webhooks** (`webhooks.rs`) — Incoming webhook handlers
- **Middleware** (`middleware.rs`) — CORS, auth, logging, trusted proxy auth
- **Protocol** (`protocol.rs`) — ACP protocol handlers
- **Commands** (`commands.rs`) — Gateway control commands
- **WebSocket** (`ws.rs`) — Real-time bidirectional message streaming
- **Send Policy** (`send_policy.rs`) — Message send policy enforcement
- **Hooks** (`hooks.rs`) — Gateway-level hooks system
- **Command Provider** (`command_provider.rs`) — Command resolution and provisioning
- **Handlers** (`handlers/`) — REST API handlers for health, device pairing, admin, etc.

### Startup Flow

1. Load configuration
2. Initialize `MemoryManager` (tiered or unified based on config)
3. Initialize `ToolRegistry` with built-in + MCP tools
4. Initialize `ChannelRegistry` with configured channels
5. Start channels (`init_channels()`)
6. Start `DreamScheduler` (if tiered memory is enabled)
7. Start HTTP server (Axum)

### GatewayConfig Fields

| Category | Fields |
|----------|--------|
| Network | `host`, `port`, `tailscale_enabled`, `tailscale_domain` |
| Agent | `default_agent` |
| Channels | `channels` (HashMap) |
| Memory | `vector_memory` |
| Plugins | `plugins` |
| Hot Reload | `hot_reload` |
| ACP | `acp` |
| Cron | `cron` |
| Heartbeat | `heartbeat` |
| Security | `security` |
| Storage | `storage` |
| Providers | `providers` (HashMap) |
| Model | `model`, `model_provider` |
| MCP | `mcp` |
| Cost | `cost_guard` |
| Workspace | `workspace_dir`, `workspace_only` |
| Browser | `browser` |
| Computer | `computer` |
| Dreaming | `dreaming` |
| Standing Orders | `standing_orders` |
| Capabilities | `capabilities` |

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

```rust
pub struct GatewayConfig {
    pub host: String,
    pub port: u16,
    pub tailscale_enabled: bool,
    pub tailscale_domain: Option<String>,
    pub default_agent: AgentConfig,
    pub channels: HashMap<String, ChannelConfig>,
    pub vector_memory: VectorMemoryConfig,
    pub plugins: PluginConfig,
    pub hot_reload: HotReloadConfig,
    pub acp: AcpConfig,
    pub cron: CronConfig,
    pub heartbeat: HeartbeatConfig,
    pub security: SecurityConfig,
    pub storage: StorageConfig,
    pub providers: HashMap<String, ProviderConfig>,
    pub model: String,
    pub model_provider: String,
    pub mcp: McpSettings,
    pub cost_guard: CostGuardConfig,
    pub workspace_dir: Option<PathBuf>,
    pub workspace_only: bool,
    pub browser: BrowserConfig,
    pub computer: ComputerConfig,
    pub dreaming: MemoryDreamingConfig,
    pub standing_orders: StandingOrderConfig,
    pub capabilities: CapabilitiesConfig,
}
```

## Implemented Features

- Axum-based HTTP API with REST endpoints
- WebSocket for real-time bidirectional streaming
- Multi-channel lifecycle management
- Agent pool with spawn and lifecycle control
- Tool registry integration with built-in and MCP tools
- Plugin manager integration
- Memory manager initialization (tiered or unified)
- Dream scheduler startup
- JWT and API key authentication
- Token bucket rate limiting
- CORS and trusted proxy middleware
- Webhook handlers for external integrations
- ACP protocol handlers for subagent control
- Send policy enforcement
- Gateway-level hooks system
- Command provider for dynamic command resolution
- Health check and device pairing handlers
- Admin endpoints for provider switching and status
- Tailscale integration support
- Cost guard configuration
- Workspace boundary enforcement
- Config snapshot and diff for change tracking

