# Manta vs OpenClaw Architecture Comparison

## Executive Summary

| Aspect | Manta | OpenClaw |
|--------|-------|----------|
| **Language** | Rust | TypeScript/Node.js |
| **Primary Focus** | Multi-channel AI gateway with extensible agent runtime | Personal AI assistant with rich UI integration |
| **Architecture** | Modular Rust crates with async/await | Plugin-based TypeScript with ESM |
| **Channels** | 6 (Telegram, Discord, Slack, WhatsApp, QQ, Lark/Feishu) | 20+ (including Signal, iMessage, WebChat, etc.) |
| **Deployment** | Single binary, daemon mode | Node.js app, gateway daemon + CLI |

---

## 1. Architecture Overview

### Manta
```
manta/
├── src/
│   ├── gateway/          # Gateway control plane (Axum WebSocket/HTTP)
│   ├── agent/            # Agent runtime with planner & prompt builder
│   ├── channels/         # Channel abstractions (trait-based)
│   ├── model_router/     # Multi-provider LLM routing with circuit breaker
│   ├── canvas/           # A2UI dynamic UI generation
│   ├── tools/            # Built-in tool registry
│   ├── memory/           # SQLite-based persistence
│   ├── tailscale/        # Tailscale remote access
│   └── cli.rs            # CLI commands
├── Cargo.toml            # Workspace configuration
└── assets/               # Web terminal HTML
```

**Key Design:**
- Single async runtime (Tokio)
- Trait-based channel abstraction
- Arc<RwLock<>> for shared state
- mpsc channels for agent communication
- Circuit breaker pattern for resilience

### OpenClaw
```
openclaw/
├── src/
│   ├── agents/           # Agent runtime (550+ files)
│   ├── gateway/          # Gateway control plane (245+ files)
│   ├── channels/         # Channel abstractions
│   ├── telegram/         # Telegram implementation
│   ├── discord/          # Discord implementation
│   ├── slack/            # Slack implementation
│   ├── whatsapp/         # WhatsApp implementation
│   ├── signal/           # Signal implementation
│   ├── imessage/         # iMessage/BlueBubbles
│   ├── web/              # Web chat interface
│   ├── memory/           # Vector DB + embeddings
│   ├── routing/          # Sophisticated route resolution
│   ├── sessions/         # Session management
│   ├── acp/              # Agent Control Plane
│   ├── canvas-host/      # Live Canvas A2UI
│   ├── plugins/          # Plugin SDK
│   ├── browser/          # Chrome automation
│   ├── tts/              # Text-to-speech
│   └── media/            # Media pipeline
├── extensions/           # Extension packages
├── skills/               # Built-in skills
└── apps/                 # Companion mobile apps
```

**Key Design:**
- Plugin-based architecture with jiti runtime loading
- ACP (Agent Control Plane) for session orchestration
- Multi-level caching for bindings and routes
- Event-driven with WebSocket events
- Sophisticated allowlist/mention gating

---

## 2. Gateway / Control Plane

| Feature | Manta | OpenClaw |
|---------|-------|----------|
| **HTTP Framework** | Axum (Rust) | Express + WebSocket (Node.js) |
| **WebSocket** | Native Axum WebSocket | ws library with custom protocol |
| **API Style** | RESTful with JSON | RESTful + WebSocket events |
| **Authentication** | Localhost/Tailscale restriction + optional API key | OAuth, API keys, rate limiting |
| **Rate Limiting** | Basic (configurable) | Sophisticated with auth-rate-limit.ts |
| **Middleware** | Tower middleware chain | Express middleware stack |
| **Control UI** | Web terminal (HTML/JS) | Full web control interface |
| **Config Reload** | Restart required | Hot reload (config-reload.ts) |
| **Boot Sequence** | Simple async init | Multi-stage boot with health checks |

### Manta Gateway Routes
```rust
// Public tier (webhooks)
/webhooks/whatsapp
/webhooks/telegram/:token
/webhooks/feishu

// Admin tier (localhost/Tailscale only)
/api/v1/agents          # Agent management
/api/v1/channels        # Channel listing
/api/v1/sessions/:id/messages  # Send with provider override
/api/v1/providers       # Provider management
/api/v1/models          # Model aliases
/api/v1/canvas          # A2UI canvas
/ws                     # WebSocket events
```

### OpenClaw Gateway Features
- Full-duplex WebSocket control plane
- Web-based control interface (control-ui.ts)
- Channel health monitoring
- Hooks system for extensibility
- Event handling with typed events
- CSP and security headers

---

## 3. Agent System

| Feature | Manta | OpenClaw |
|---------|-------|----------|
| **Runtime** | Tokio async with mpsc channels | ACP (Agent Control Plane) |
| **Spawning** | spawn_agent() with AgentHandle | acp-spawn.ts with session actor queue |
| **Modes** | Single persistent mode | "run" (one-shot) vs "session" (persistent) |
| **Subagents** | Not yet implemented | Full support with thread binding |
| **Planner** | TaskPlanner with LLM decomposition | Integrated into ACP |
| **Prompt Builder** | Dynamic prompt building with context | Model overrides, level overrides |
| **Memory Files** | AGENTS.md, TOOLS.md support | Extensive memory system |

### Manta Agent
```rust
pub struct Agent {
    config: AgentConfig,
    provider: Arc<dyn Provider>,
    tool_registry: Arc<ToolRegistry>,
    planner: Option<TaskPlanner>,
    todo_store: Arc<TodoStore>,
}

pub struct AgentHandle {
    pub id: String,
    pub config: AgentConfig,
    pub tx: mpsc::Sender<AgentCommand>,
    pub busy: bool,
    pub agent: Arc<Agent>,
}

pub enum AgentCommand {
    ProcessMessage { session_id, message, user_id, channel },
    Cancel,
    UpdateConfig(AgentConfig),
    Shutdown,
}
```

### OpenClaw Agent
```typescript
// ACP Session Manager with actor queue
class ACPSessionManager {
  sessionActorQueue: SessionActorQueue
  runtimeControls: RuntimeControls
  spawnSubagent(mode: "run" | "session"): Promise<Agent>
}

// Thread bindings for persistence
interface PersistentBinding {
  threadId: string
  agentId: string
  mode: "oneshot" | "persistent"
}
```

---

## 4. Session Management

| Feature | Manta | OpenClaw |
|---------|-------|----------|
| **Storage** | SQLite (SqliteMemoryStore) | File-based with transcripts |
| **Session Key** | Simple format: "{channel}:{user_id}" | Normalized with account/agent scoping |
| **Routing** | HashMap<session_id, agent_id> | Sophisticated resolve-route.ts (600+ lines) |
| **Group Sessions** | Basic support | Full group.ts implementation |
| **Transcripts** | ✅ `transcript.rs` with multi-format export | Full transcript.ts |
| **Artifacts** | ✅ `artifacts.rs` with session-bound lifecycle | artifacts.ts |
| **Disk Budget** | ✅ `disk_budget.rs` with per-session quotas | disk-budget.ts enforcement |
| **Send Policy** | Basic | send-policy.ts with rich rules |

### Manta Session Routing
```rust
pub struct GatewayState {
    pub session_routing: Arc<RwLock<HashMap<String, String>>>,
}

async fn resolve_agent_for_session(state: &Arc<GatewayState>, session_id: &str) -> String {
    let routing = state.session_routing.read().await;
    routing.get(session_id).cloned().unwrap_or_else(|| "default".to_string())
}
```

### OpenClaw Session Resolution
```typescript
// Sophisticated binding matching
interface RouteResolution {
  peer: string
  guild?: string
  team?: string
  account: string
  channel: string
  scope: "dm" | "channel" | "thread"
  roleBased?: boolean
}

// Caching system for evaluated bindings
const bindingCache = new Map<string, ResolvedBinding>()
```

---

## 5. Model Routing & Provider Switching

| Feature | Manta | OpenClaw |
|---------|-------|----------|
| **Circuit Breaker** | ✅ Full implementation (Closed/Open/HalfOpen) | ❌ Not implemented |
| **Health Tracking** | Latency, failures, successes | Provider usage tracking |
| **Fallback Chains** | ✅ Dynamic at runtime | ✅ Configured |
| **Per-Request Override** | ✅ Provider + model alias | ✅ Model overrides |
| **Runtime API** | ✅ Full REST API | CLI commands |
| **Auth Profiles** | ✅ Multi-key rotation with cooldown/failover | ✅ auth-profiles/ with rotation |
| **Provider Types** | Anthropic, OpenAI, Azure, Ollama | GitHub Copilot, Google, Anthropic, OpenAI |

### Manta ModelRouter
```rust
pub struct ModelRouter {
    config: RwLock<ModelRouterConfig>,
    providers: RwLock<HashMap<String, Arc<dyn Provider + Send + Sync>>>,
    health: RwLock<HashMap<String, ProviderHealth>>,
    fallback_chains: RwLock<HashMap<String, Vec<FallbackEntry>>>,
}

pub struct ProviderHealth {
    pub state: CircuitState,  // Closed, Open, HalfOpen
    pub failures: u32,
    pub successes: u64,
    pub avg_latency_ms: u64,
}

// Runtime methods
async fn switch_default_model(&self, alias: &str) -> Result<()>
async fn enable/disable_provider(&self, name: &str) -> Result<()>
async fn complete_with_provider(&self, provider: &str, ...) -> Result<...>
```

### OpenClaw Provider Management
```typescript
// Provider usage tracking
interface ProviderUsage {
  profile: AuthProfile
  cooldown: Date
  failover: boolean
}

// Auth profile rotation
class AuthProfileManager {
  profiles: AuthProfile[]
  currentIndex: number
  rotate(): AuthProfile
}
```

---

## 6. Tool Execution

| Feature | Manta | OpenClaw |
|---------|-------|----------|
| **Registry** | ToolRegistry with Box<dyn Tool> | ToolCatalog with policy enforcement |
| **Policy** | Basic allowlist | tool-policy.ts with granular rules |
| **Bash Execution** | ✅ ShellTool | ✅ bash-tools.exec-runtime.ts |
| **Browser** | ✅ Chromiumoxide (optional) | ✅ Dedicated browser/ module |
| **File Operations** | Read, Write, Edit, Glob, Grep | Extensive file operations |
| **Web Tools** | Search, Fetch | Similar + more |
| **Canvas/A2UI** | ✅ CanvasComponent + CanvasManager + outbound pipeline wired | ✅ Full canvas-host/ |
| **Subagent Tools** | ✅ `AcpSpawnTool` + `DelegateTool` | ✅ Session spawning tools |
| **Plugin Tools** | ✅ `PluginToolWrapper` + dynamic registration | ✅ Plugin SDK |
| **Dangerous Tools** | Basic validation | Security audit system |

### Manta Tools
```rust
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

// Built-in tools
FileReadTool, FileWriteTool, FileEditTool
ShellTool, CodeExecutionTool
WebSearchTool, WebFetchTool
TodoTool, CronTool, TimeTool
BrowserTool (optional feature)
```

### OpenClaw Tools
```typescript
// Tool policy enforcement
interface ToolPolicy {
  allowedTools: string[]
  dangerousTools: string[]
  requireConfirmation: boolean
}

// Categories
bash-tools, browser-tools, channel-tools
openclaw-tools (subagents), pi-tools (canvas)
plugin runtime tools
```

---

## 7. Memory / Persistence

| Feature | Manta | OpenClaw |
|---------|-------|----------|
| **Database** | SQLite (sqlx) | Multiple: builtin, qmd, lancedb |
| **Chat History** | ✅ messages table | transcripts.ts |
| **Embeddings** | ✅ Local GGUF + API providers | ✅ Full embedding system |
| **Vector DB** | ✅ pgvector, SQLite-vec | ✅ QMD, LanceDB support |
| **Hybrid Search** | ✅ Vector + FTS5 | Partial |
| **Session Files** | ✅ `SessionFileManager` (session-scoped FS) | ✅ session-files.ts |
| **Memory Files** | AGENTS.md, TOOLS.md | SOUL.md, IDENTITY.md, BOOTSTRAP.md |
| **Chunking** | ✅ TextChunker | ✅ embedding-chunk-limits.ts |
| **Batch Processing** | ✅ BatchEmbeddingProcessor | ✅ Gemini, OpenAI, Voyage batching |

### Manta Memory
```rust
pub struct SqliteMemoryStore {
    pool: SqlitePool,
}

// Tables
conversations, messages, agent_memory
```

### OpenClaw Memory
```typescript
// Backend configuration
interface MemoryBackend {
  type: "builtin" | "qmd" | "lancedb"
  embeddingModel: string
  collections: string[]
}

// Batching support
class GeminiBatchProcessor
class OpenAIBatchProcessor
class VoyageBatchProcessor
```

---

## 8. Multi-Channel Support

| Feature | Manta | OpenClaw |
|---------|-------|----------|
| **Total Channels** | 6 | 20+ |
| **Architecture** | Trait-based (Channel trait) | Plugin-based with dock.ts |
| **Telegram** | ✅ teloxide | ✅ grammY |
| **Discord** | ✅ serenity | ✅ discord.js |
| **Slack** | Stub (reqwest) | ✅ Bolt |
| **WhatsApp** | ✅ Webhooks + HMAC | ✅ Baileys |
| **Signal** | ❌ Not implemented | ✅ signal-cli |
| **iMessage** | ❌ Not implemented | ✅ BlueBubbles |
| **WebChat** | Web terminal | ✅ Full web interface |
| **QQ** | Stub | Extension |
| **Lark/Feishu** | ✅ Re-export from Lark | Extension |
| **Allowlists** | Basic | Sophisticated allowlist-match.ts |
| **Mention Gating** | ✅ `MentionGate` with wildcard patterns | ✅ mention-gating.ts |
| **Command Gating** | ✅ `CommandGate` with user levels | ✅ command-gating.ts |

### Manta Channel Trait
```rust
#[async_trait]
pub trait Channel: Send + Sync {
    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn send_message(&self, request: OutgoingMessage) -> Result<()>;
    fn channel_type(&self) -> ChannelType;
}
```

### OpenClaw Channel Dock
```typescript
// Channel registry with capabilities
interface ChannelDock {
  register(channel: ChannelPlugin): void
  getCapabilities(channelType: string): ChannelCapabilities
  buildThreadingContext(channel: string, message: Message): ThreadContext
}

// Mention and command gating
mentionGating: MentionGatingConfig
commandGating: CommandGatingConfig
```

---

## 9. Security

| Feature | Manta | OpenClaw |
|---------|-------|----------|
| **DM Pairing** | ✅ `PairingStore` with code-based approval | ✅ Full pairing system |
| **Allowlist Matching** | Basic | Pattern matching with normalization |
| **Webhook Verification** | ✅ HMAC-SHA256 | ✅ Signature verification |
| **Audit Logging** | ✅ Persistent audit log (SQLite + in-memory) | ✅ Comprehensive audit.ts |
| **Tool Auditing** | Basic | audit-tool-policy.ts |
| **CSP Headers** | ✅ Route-aware CSP with nonces | ✅ control-ui-csp.ts |
| **Sandboxing** | ✅ `SandboxedTool` with path/network/timeout controls | ✅ Sandbox modes for tools |
| **Rate Limiting** | ✅ Multi-tier sliding window + legacy token bucket | Sophisticated per-channel |

---

## 10. Unique Features Comparison

### Manta Unique Features
1. **Circuit Breaker Pattern** - Automatic provider failover with health tracking
2. **Tailscale Integration** - Built-in remote access via Tailscale
3. **Rust Performance** - Single binary, low memory footprint
4. **Task Planner** - LLM-based natural language task decomposition
5. **Dynamic Prompt Builder** - Context-aware prompt construction
6. **Feature Flags** - Compile-time channel selection (Cargo features)
7. **Hybrid Search** - Vector + FTS5 with MMR re-ranking
8. **Inbound/Outbound Pipeline DAG** - OpenClaw-aligned skeleton with debounce → media → queue → router → trajectory → canvas → sse → side effects

### OpenClaw Unique Features
1. **ACP (Agent Control Plane)** - Sophisticated session orchestration
2. **Canvas Host** - Live A2UI with visual workspace manipulation
3. **Voice/TTS** - Text-to-speech and voice wake
4. **Media Pipeline** - Images, audio, video processing (Manta has image routing)
5. **Plugin SDK** - Extensible plugin architecture with jiti
6. **Mobile Apps** - iOS and Android companion apps
7. **Browser Control** - Dedicated Chrome automation
8. **Subagent Spawning** - Thread-bound persistent subagents
9. **Vector DB** - QMD and LanceDB embedding support
10. **Hot Config Reload** - Runtime configuration updates

---

## 11. Technology Stack

| Component | Manta | OpenClaw |
|-----------|-------|----------|
| **Language** | Rust 1.75+ | TypeScript/Node.js 22+ |
| **Runtime** | Tokio async | Node.js event loop |
| **Build** | Cargo | pnpm + TypeScript |
| **HTTP** | Axum | Express |
| **WebSocket** | tokio-tungstenite | ws library |
| **Database** | SQLite (sqlx) | Multiple (configurable) |
| **Testing** | cargo test + mockall | Vitest |
| **Linting** | clippy + rustfmt | oxlint + oxfmt |
| **Process** | daemonize crate | launchd/systemd |

---

## 12. Feature Matrix

| Feature | Manta | OpenClaw |
|---------|-------|----------|
| **Core Gateway** | ✅ | ✅ |
| **WebSocket API** | ✅ | ✅ |
| **REST API** | ✅ | ✅ |
| **Multi-Agent** | ✅ | ✅ |
| **Agent Spawning** | ✅ | ✅ |
| **Session Management** | AgentRouter + QueueModeResolver + session buffers | Advanced |
| **Model Aliases** | ✅ | ✅ |
| **Fallback Chains** | ✅ | ✅ |
| **Circuit Breaker** | ✅ | ❌ |
| **Provider Health** | ✅ | Basic |
| **Per-Request Override** | ✅ | ✅ |
| **Runtime Provider API** | ✅ | CLI only |
| **Natural Language Planning** | ✅ | Partial |
| **Dynamic Prompt Building** | ✅ | Partial |
| **Browser Automation** | ✅ | ✅ |
| **Canvas/A2UI** | ✅ | ✅ |
| **File Tools** | ✅ | ✅ |
| **Shell Execution** | ✅ | ✅ |
| **Todo Management** | ✅ | ❌ |
| **Cron Jobs** | ✅ | ✅ |
| **Side Effects** | ✅ (memory, cron, webhook, analytics) | ✅ |
| **Queue Mode** | ✅ (Interrupt/Steer/FollowUp/Collect/Normal) | ✅ |
| **SSE Streaming** | ✅ Per-session broadcast | ✅ |
| **SQLite Memory** | ✅ | Optional |
| **Vector DB** | ✅ (pgvector, SQLite-vec) | ✅ |
| **Embeddings** | ✅ (local GGUF + API providers) | ✅ |
| **Multi-Channel (6+)** | 6 | 20+ |
| **Telegram** | ✅ | ✅ |
| **Discord** | ✅ | ✅ |
| **Slack** | Stub | ✅ |
| **WhatsApp** | ✅ | ✅ |
| **Signal** | ❌ | ✅ |
| **iMessage** | ❌ | ✅ |
| **WebChat** | Terminal | Full UI |
| **DM Pairing** | ✅ `PairingStore` with code approval | ✅ |
| **Allowlists** | Basic | Advanced |
| **Mention Gating** | ✅ `MentionGate` with wildcard patterns | ✅ |
| **Command Gating** | ✅ `CommandGate` with user levels | ✅ |
| **Voice/TTS** | ❌ | ✅ |
| **Media Pipeline** | ✅ Image routing via ModelRouter | ✅ |
| **Plugin System** | ✅ WASM plugins + dynamic tools/channels | ✅ |
| **Mobile Apps** | ❌ | ✅ |
| **Hot Reload** | ✅ `HotReloadManager` + `PluginManager` reload | ❌ |
| **Tailscale** | ✅ | ❌ |
| **Single Binary** | ✅ | ❌ |
| **Cross-Platform** | ✅ | macOS focused |

---

## 13. Code Size Comparison

| Metric | Manta | OpenClaw |
|--------|-------|----------|
| **Total Lines** | ~15,000 | ~100,000+ |
| **Source Files** | ~50 | ~2,000+ |
| **Agent System** | ~1,500 lines | ~20,000 lines |
| **Gateway** | ~1,200 lines | ~15,000 lines |
| **Channels** | ~800 lines | ~30,000 lines |
| **Memory** | ~500 lines | ~10,000 lines |

---

## Summary

**Manta** is a lean, Rust-based multi-channel AI gateway focused on:
- Performance and reliability (circuit breaker, single binary)
- Runtime provider management (hot switching, health monitoring)
- Extensible architecture (traits, feature flags)
- Modern async patterns (Tokio, Axum)
- **OpenClaw-aligned skeleton** with full inbound/outbound pipeline DAG

**OpenClaw** is a comprehensive TypeScript AI assistant platform with:
- Rich UI integration (Canvas, WebChat, mobile apps)
- Sophisticated routing and session management
- Voice and media capabilities
- Extensive plugin ecosystem
- macOS/iOS ecosystem integration

Manta excels at being a lightweight, reliable gateway with modern Rust patterns and now matches OpenClaw's core pipeline architecture. OpenClaw excels at being a full-featured personal assistant with rich UI, voice, and mobile capabilities.

---

## 14. Detailed Module-by-Module Comparison

### 14.1 Gateway / Web Control Plane

| Dimension | OpenClaw | Manta |
|---|---|---|
| **Framework** | Express + ws (Node.js) | Axum + tokio-tungstenite (Rust) |
| **WebSocket** | Full-duplex control plane, custom protocol | Axum native WebSocket, `/ws` endpoint |
| **REST API** | Complete, with control interface | `/api/v1/*` management endpoints |
| **Authentication** | OAuth + API Key + rate limiting | localhost/Tailscale restriction + optional API Key |
| **Rate Limiting** | Sophisticated `auth-rate-limit.ts` | Basic configurable |
| **Middleware** | Express middleware stack | Tower middleware chain |
| **Control UI** | Full web control interface | Web Terminal (HTML/JS) |
| **Config Hot Reload** | ✅ `config-reload.ts` | ✅ `HotReloadManager` with type-safe handlers |
| **Boot Sequence** | Multi-stage with health checks | Single-thread async init |
| **CSP Security Headers** | ✅ `control-ui-csp.ts` | ✅ Route-aware CSP with nonces |

**Gap**: Manta lacks full web control interface.

---

### 14.2 Agent Runtime

| Dimension | OpenClaw | Manta |
|---|---|---|
| **Runtime** | ACP (Agent Control Plane) + actor queue | Tokio async + mpsc channels |
| **Modes** | `run` (one-shot) / `session` (persistent) | ✅ `run` / `session` via `ExecutionMode` |
| **Subagent Spawning** | ✅ Thread-bound persistent subagents | ✅ `AcpControlPlane` + `DelegateTool` |
| **Planner** | Integrated into ACP | ✅ `TaskPlanner` (LLM decomposition) |
| **Prompt Builder** | Model overrides, level overrides | ✅ Dynamic context building |
| **Memory Files** | SOUL.md, IDENTITY.md, BOOTSTRAP.md | AGENTS.md, TOOLS.md |
| **Context Compression** | Integrated | ✅ `ContextCompressor` |
| **Cost Guard** | Basic | ✅ `CostGuard` (daily limit + hourly action rate) |

**Gap**: Manta lacks ACP-level session orchestration depth (runtime controls exist but not fully wired to external triggers).

---

### 14.3 Inbound Pipeline (Message Ingress)

| Stage | OpenClaw | Manta | Status |
|---|---|---|---|
| **Debounce** | ✅ Key debounce | ✅ `InboundDebouncer` + flush mechanism | ✅ Aligned |
| **Media Understanding** | Full media pipeline (vision/STT/video) | ✅ `MediaUnderstandingPipeline` — images routed via `ModelRouter` to vision providers | ✅ Aligned |
| **AutoReply Dispatch** | `send-policy.ts` rich rules | ✅ `AutoReplyDispatch` + suppress logic | ✅ Aligned |
| **Queue Mode** | ✅ Interrupt/Steer/FollowUp/Collect/Normal | ✅ `QueueModeResolver` — 5 modes + session-level heuristics + message buffering | ✅ Aligned |
| **Agent Router** | `resolve-route.ts` (600+ lines, complex binding) | ✅ `AgentRouter` — workspace-aware multi-agent routing | ✅ Aligned |

**All inbound stages are now skeleton-aligned.**

---

### 14.4 Outbound Pipeline (Response Egress)

| Stage | OpenClaw | Manta | Status |
|---|---|---|---|
| **Trajectory** | Execution trace recording | ✅ `TrajectoryLog` — built by agent, forwarded through pipeline | ✅ Aligned |
| **Canvas** | Live Canvas A2UI | ✅ `CanvasComponent` JSON detection in `DefaultOutboundPipeline` → applied via `CanvasManager` | ✅ Aligned |
| **SSE** | Streaming event push | ✅ `SseStreamer` — per-session broadcast, subscriber tracking, GC | ✅ Aligned |
| **Reply Dispatcher** | Route by channel | ✅ `ReplyDispatcher` — multi-channel routing | ✅ Aligned |
| **Side Effects** | Memory, cron, webhooks | ✅ `SideEffectExecutor` — MemoryStore/CronSchedule/Webhook/Analytics all implemented + runtime context wired | ✅ Aligned |

**All outbound stages are now skeleton-aligned.**

---

### 14.5 Model Router / LLM Providers

| Dimension | OpenClaw | Manta |
|---|---|---|
| **Multi-Provider** | Anthropic, OpenAI, GitHub Copilot, Google | Anthropic, OpenAI, Azure, Ollama |
| **Model Aliases** | ✅ | ✅ |
| **Fallback Chains** | ✅ Configured | ✅ Dynamic runtime |
| **Circuit Breaker** | ❌ | ✅ `CircuitState` (Closed/Open/HalfOpen) |
| **Health Tracking** | Provider usage stats | ✅ Latency, failures, successes |
| **Auth Profile Rotation** | ❌ | ✅ `auth-profiles/` |
| **Runtime Switching** | CLI commands | ✅ REST API |
| **Provider SDK** | Dynamic discovery | ✅ `ProviderSdk` + `sync_from_model_router()` |

**Manta advantage**: Circuit breaker, auth profile rotation, runtime REST API, health tracking.
**OpenClaw advantage**: More providers (GitHub Copilot, Google).

---

### 14.6 Tool System

| Dimension | OpenClaw | Manta |
|---|---|---|
| **Registry** | `ToolCatalog` + policy | `ToolRegistry` |
| **Policy** | `tool-policy.ts` granular rules | Basic allowlist |
| **File Operations** | Extensive | ✅ read/write/edit/glob/grep |
| **Shell** | ✅ | ✅ `ShellTool` |
| **Browser** | Dedicated `browser/` module | ✅ `BrowserTool` (chromiumoxide) |
| **Web Search** | Rich | ✅ search/fetch |
| **Canvas Tools** | ✅ `pi-tools` | ✅ via `CanvasManager` |
| **Subagent Tools** | ✅ Session spawning | ✅ `AcpSpawnTool` + `DelegateTool` |
| **Plugin Tools** | ✅ Plugin SDK | ✅ `PluginToolWrapper` + dynamic registration |
| **Security Audit** | `audit-tool-policy.ts` | Basic validation |
| **Sandbox** | ✅ Sandbox modes | ✅ `SandboxedTool` with path/network/timeout controls |
| **Tool SDK** | Dynamic tool pack | ✅ `ToolSdk` + `sync_from_tool_registry()` |
| **Hooks** | Event hooks | ✅ `ToolHooks` |
| **Approval Queue** | Human-in-the-loop | ✅ `ApprovalQueue` |

**Gap**: None — subagent and plugin tools are implemented.

---

### 14.7 Memory / Persistence

| Dimension | OpenClaw | Manta |
|---|---|---|
| **Database** | Multiple backends (builtin/qmd/lancedb) | SQLite (sqlx) + WAL + FTS5 |
| **Chat History** | `transcripts.ts` | ✅ `messages` table |
| **Semantic Memory** | Vector DB | ✅ `Memory` table + embedding |
| **Vector Search** | QMD / LanceDB | ✅ `VectorMemoryService` (pgvector, SQLite-vec) |
| **Hybrid Search** | Partial | ✅ Vector + FTS5 + MMR re-ranking |
| **Embeddings** | Gemini/OpenAI/Voyage batch | ✅ `LocalGgufEmbeddingProvider` + API providers |
| **Chunking** | `embedding-chunk-limits.ts` | ✅ `TextChunker` |
| **Batch Processing** | Gemini/OpenAI/Voyage | ✅ `BatchEmbeddingProcessor` |
| **Session Files** | `session-files.ts` | ✅ `SessionFileManager` |
| **MemoryManager** | No unified orchestrator | ✅ `MemoryManager` (observe/retrieve/session_context) |
| **Workspace State** | Basic | ✅ `WorkspaceManager` + `WorkspaceState` |

**Manta advantage**: Hybrid search (vector + FTS5 + MMR), unified `MemoryManager`.
**OpenClaw advantage**: More backend choices, session file system.

---

### 14.8 Channels (Messaging)

| Channel | OpenClaw | Manta |
|---|---|---|
| **Telegram** | ✅ grammY | ✅ teloxide |
| **Discord** | ✅ discord.js | ✅ serenity |
| **Slack** | ✅ Bolt | stub (reqwest) |
| **WhatsApp** | ✅ Baileys | ✅ Webhooks + HMAC |
| **Signal** | ✅ signal-cli | ❌ |
| **iMessage** | ✅ BlueBubbles | ❌ |
| **WebChat** | Full web interface | Web Terminal |
| **QQ** | Extension | stub |
| **Lark/Feishu** | Extension | ✅ |
| **Total Channels** | 20+ | 6 |
| **Architecture** | Plugin-based `dock.ts` | Trait-based (`Channel` trait) |
| **ChannelExtension** | Unified interface | ✅ `ChannelExtension` trait + `TelegramChannelExtension` |
| **Mention Gating** | ✅ `mention-gating.ts` | ✅ `MentionGate` with wildcard patterns |
| **Command Gating** | ✅ `command-gating.ts` | ✅ `CommandGate` with user levels |
| **Allowlist** | Sophisticated pattern matching | Basic |

**Gap**: Manta lacks Signal/iMessage, sophisticated allowlist.

---

### 14.9 Canvas / A2UI

| Dimension | OpenClaw | Manta |
|---|---|---|
| **Component System** | `canvas-host/` (full) | ✅ `CanvasComponent` enum (16 component types) |
| **Session Management** | Full | ✅ `CanvasSession` + `CanvasManager` |
| **WebSocket Protocol** | Custom | ✅ `CanvasWebSocketHandler` |
| **Real-time Updates** | ✅ | ✅ `broadcast::Sender<CanvasUpdate>` |
| **Helper Functions** | Rich | ✅ create_form/create_progress/create_alert/etc. |
| **Outbound Integration** | Integrated | ✅ `DefaultOutboundPipeline` detects JSON and applies |

**Status**: Skeleton aligned. OpenClaw's UI is richer.

---

### 14.10 SSE / Real-time Streaming

| Dimension | OpenClaw | Manta |
|---|---|---|
| **Event Types** | token/tool/start/end/error | ✅ Token/ToolStart/ToolEnd/Done/Error/Heartbeat |
| **Broadcast** | Global / session-level | ✅ Per-session `broadcast::Sender` |
| **Subscription Management** | Client connections | ✅ `subscribe()` + receiver count |
| **Back-pressure** | Yes | ✅ Channel capacity (256) |
| **GC** | Automatic | ✅ `gc()` cleans sessions with no receivers |
| **Endpoint** | WebSocket events | ✅ `/api/events` (axum SSE) |

**Status**: Fully implemented.

---

### 14.11 Cron / Scheduled Tasks

| Dimension | OpenClaw | Manta |
|---|---|---|
| **Scheduler** | Basic cron | ✅ `CronScheduler` (production-grade) |
| **Trigger Types** | Cron only | ✅ At / Every / Cron three `Schedule` types |
| **Execution Targets** | Shell / agent / webhook | ✅ `ExecutionTarget` (Shell/Agent) |
| **Delivery Modes** | None | ✅ None / Announce / Webhook |
| **Retry** | None | ✅ `RetryConfig` |
| **State Tracking** | None | ✅ `JobState` (next_run/last_run/run_count/errors) |
| **Crash Recovery** | None | ✅ Persist to JSON + recover on startup |
| **Announce Integration** | None | ✅ Broadcast via `event_tx` to Gateway |
| **Side Effect Trigger** | None | ✅ `CronSchedule` side effect from outbound pipeline |

**Manta advantage**: Production-grade cron scheduler. OpenClaw's cron is simpler.

---

### 14.12 Security

| Dimension | OpenClaw | Manta |
|---|---|---|
| **DM Pairing** | ✅ Full pairing system | ✅ `PairingStore` with code-based approval |
| **Allowlist** | Sophisticated pattern matching | Basic |
| **Webhook Verification** | ✅ Signature verification | ✅ HMAC-SHA256 |
| **Audit Logging** | `audit.ts` comprehensive | ✅ Persistent audit log (SQLite + in-memory) |
| **Tool Auditing** | `audit-tool-policy.ts` | Basic |
| **CSP** | ✅ | ✅ Route-aware CSP with nonces |
| **Rate Limiting** | Sophisticated per-channel | ✅ Multi-tier sliding window + legacy token bucket |
| **Sandbox** | ✅ Sandbox modes | ✅ `SandboxedTool` with path/network/timeout controls |
| **Sliding Window** | Basic | ✅ `SlidingWindow` rate limiter |

**Gap**: None — DM pairing system is implemented.

---

### 14.13 Plugin System

| Dimension | OpenClaw | Manta |
|---|---|---|
| **Runtime** | jiti (ESM hot reload) | ✅ WASM hot reload (`PluginRuntime`) |
| **SDK** | Full Plugin SDK | ✅ `PluginManager` + manifest + hooks |
| **Channel Plugins** | Dynamic registration | ✅ `PluginChannelRegistry` + WASM channels |
| **Tool Plugins** | Dynamic registration | ✅ `register_dynamic` + `PluginToolWrapper` |
| **WASM** | None | ✅ wasmtime (feature flag) |

**Gap**: Manta's plugin system is WASM-based (sandboxed) vs OpenClaw's jiti ESM (full Node.js access). Feature parity exists; runtime model differs.

---

### 14.14 Session Management

| Dimension | OpenClaw | Manta |
|---|---|---|
| **Storage** | File + transcript | SQLite |
| **Routing** | `resolve-route.ts` (600+ lines) | `AgentRouter` + `QueueModeResolver` |
| **Session Key** | Normalized + account/agent scope | `{channel}:{user_id}` |
| **Group Sessions** | `group.ts` full implementation | Basic support |
| **Transcripts** | `transcript.ts` | ✅ `TranscriptManager` |
| **Artifacts** | `artifacts.ts` | ✅ `ArtifactStore` |
| **Disk Budget** | `disk-budget.ts` | ✅ `DiskBudgetManager` |
| **Session Buffers** | Basic | ✅ `session_message_buffer` for FollowUp/Collect |

**Gap**: Manta's session management is simpler; lacks group sessions and transcript file system.

---

## 15. Module Maturity Scorecard

| Module | OpenClaw | Manta | Gap |
|---|---|---|---|
| **Gateway** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | Missing control UI |
| **Agent Runtime** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ACP exists; lacks deep session orchestration |
| **Inbound Pipeline** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ✅ Skeleton aligned |
| **Outbound Pipeline** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ✅ Skeleton aligned |
| **Model Router** | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | Manta has circuit breaker advantage |
| **Tool System** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | Missing subagent/plugin tools |
| **Memory** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | Missing session files, multi-backend |
| **Channels** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | Missing Signal/iMessage/mention gating |
| **Canvas** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | Skeleton aligned, UI richness gap |
| **SSE** | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ✅ Fully implemented |
| **Cron** | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ | Manta more production-grade |
| **Security** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | Missing mention gating |
| **Plugin** | ⭐⭐⭐⭐⭐ | ⭐ | Far from mature |
| **Session Mgmt** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | Missing transcripts/artifacts |

---

## 16. Manta's Core Advantages

1. **Circuit Breaker** — Automatic provider failover with health tracking (OpenClaw lacks this)
2. **Rust Single Binary** — Low memory footprint, cross-platform, no runtime dependency
3. **Production Cron** — At/Every/Cron schedules with retry, crash recovery, state tracking
4. **Hybrid Search** — Vector + FTS5 + MMR re-ranking in unified `MemoryManager`
5. **Pipeline DAG** — OpenClaw-aligned skeleton: debounce → media → queue → router → trajectory → canvas → sse → side effects
6. **Tailscale** — Built-in remote access (OpenClaw lacks this)
7. **Task Planner** — LLM-based natural language task decomposition
8. **Runtime Provider API** — Hot switch providers via REST (OpenClaw only CLI)

## 17. OpenClaw's Core Advantages

1. **ACP** — Sophisticated session orchestration with actor queue
2. **Rich Channels** — 20+ channels including Signal, iMessage
3. **Plugin Ecosystem** — jiti runtime loading, full SDK
4. **Voice/TTS** — Text-to-speech and voice wake
5. **Mobile Apps** — iOS and Android companion apps
6. **Hot Config Reload** — Runtime configuration updates without restart
7. **Subagent Spawning** — Thread-bound persistent subagents
8. **Security** — DM pairing, audit logging, sandbox modes, CSP
9. **macOS Integration** — Deep ecosystem integration (BlueBubbles, launchd)
