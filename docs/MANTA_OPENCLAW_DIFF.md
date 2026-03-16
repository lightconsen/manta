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
| **Transcripts** | Not implemented | Full transcript.ts |
| **Artifacts** | Not implemented | artifacts.ts |
| **Disk Budget** | Not implemented | disk-budget.ts enforcement |
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
| **Auth Profiles** | ❌ Not implemented | ✅ auth-profiles/ with rotation |
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
| **Canvas/A2UI** | ✅ CanvasComponent enum | ✅ Full canvas-host/ |
| **Subagent Tools** | ❌ Not implemented | ✅ Session spawning tools |
| **Plugin Tools** | ❌ Not implemented | ✅ Plugin SDK |
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
| **Embeddings** | ❌ Not implemented | ✅ Full embedding system |
| **Vector DB** | ❌ Not implemented | ✅ QMD, LanceDB support |
| **Session Files** | ❌ Not implemented | ✅ session-files.ts |
| **Memory Files** | AGENTS.md, TOOLS.md | SOUL.md, IDENTITY.md, BOOTSTRAP.md |
| **Chunking** | ❌ Not implemented | ✅ embedding-chunk-limits.ts |
| **Batch Processing** | ❌ Not implemented | ✅ Gemini, OpenAI, Voyage batching |

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
| **Mention Gating** | ❌ Not implemented | ✅ mention-gating.ts |
| **Command Gating** | ❌ Not implemented | ✅ command-gating.ts |

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
| **DM Pairing** | ❌ Not implemented | ✅ Full pairing system |
| **Allowlist Matching** | Basic | Pattern matching with normalization |
| **Webhook Verification** | ✅ HMAC-SHA256 | ✅ Signature verification |
| **Audit Logging** | ❌ Not implemented | ✅ Comprehensive audit.ts |
| **Tool Auditing** | Basic | audit-tool-policy.ts |
| **CSP Headers** | ❌ Not implemented | ✅ control-ui-csp.ts |
| **Sandboxing** | ❌ Not implemented | ✅ Sandbox modes for tools |
| **Rate Limiting** | Basic middleware | Sophisticated per-channel |

---

## 10. Unique Features Comparison

### Manta Unique Features
1. **Circuit Breaker Pattern** - Automatic provider failover with health tracking
2. **Tailscale Integration** - Built-in remote access via Tailscale
3. **Rust Performance** - Single binary, low memory footprint
4. **Task Planner** - LLM-based natural language task decomposition
5. **Dynamic Prompt Builder** - Context-aware prompt construction
6. **Feature Flags** - Compile-time channel selection (Cargo features)

### OpenClaw Unique Features
1. **ACP (Agent Control Plane)** - Sophisticated session orchestration
2. **Canvas Host** - Live A2UI with visual workspace manipulation
3. **Voice/TTS** - Text-to-speech and voice wake
4. **Media Pipeline** - Images, audio, video processing
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
| **Session Management** | Basic | Advanced |
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
| **SQLite Memory** | ✅ | Optional |
| **Vector DB** | ❌ | ✅ |
| **Embeddings** | ❌ | ✅ |
| **Multi-Channel (6+)** | 6 | 20+ |
| **Telegram** | ✅ | ✅ |
| **Discord** | ✅ | ✅ |
| **Slack** | Stub | ✅ |
| **WhatsApp** | ✅ | ✅ |
| **Signal** | ❌ | ✅ |
| **iMessage** | ❌ | ✅ |
| **WebChat** | Terminal | Full UI |
| **DM Pairing** | ❌ | ✅ |
| **Allowlists** | Basic | Advanced |
| **Mention Gating** | ❌ | ✅ |
| **Command Gating** | ❌ | ✅ |
| **Voice/TTS** | ❌ | ✅ |
| **Media Pipeline** | ❌ | ✅ |
| **Plugin System** | ❌ | ✅ |
| **Mobile Apps** | ❌ | ✅ |
| **Hot Reload** | ❌ | ✅ |
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

**OpenClaw** is a comprehensive TypeScript AI assistant platform with:
- Rich UI integration (Canvas, WebChat, mobile apps)
- Sophisticated routing and session management
- Voice and media capabilities
- Extensive plugin ecosystem
- macOS/iOS ecosystem integration

Manta excels at being a lightweight, reliable gateway with modern Rust patterns. OpenClaw excels at being a full-featured personal assistant with rich UI and media capabilities.
