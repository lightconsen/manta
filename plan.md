# Manta - Personal AI Assistant

## Vision

Manta is a lightweight, fast, and secure Personal AI Assistant written in Rust. It combines the simplicity philosophy of NanoClaw with the performance characteristics of ZeroClaw, targeting <10MB binary size and <20MB RAM usage.

## Core Principles

1. **Lightweight**: Minimal resource footprint, fast startup
2. **Secure by Design**: Deny-by-default, explicit allowlists, sandboxed execution
3. **Modular**: Trait-driven architecture, swappable components
4. **AI-Native**: No dashboards, natural language interface
5. **Single Binary**: Easy deployment, minimal dependencies

## Architecture

```
manta/
├── src/
│   ├── main.rs           # CLI entry point
│   ├── config.rs         # Configuration management
│   ├── agent/
│   │   ├── mod.rs        # Core agent orchestration
│   │   ├── context.rs    # Conversation context management
│   │   └── router.rs     # Request routing & handling
│   ├── providers/
│   │   ├── mod.rs        # LLM provider trait
│   │   ├── openai.rs     # OpenAI provider
│   │   ├── anthropic.rs  # Anthropic provider
│   │   └── local.rs      # Local model support (ollama, etc.)
│   ├── channels/
│   │   ├── mod.rs        # Channel trait
│   │   ├── telegram.rs   # Telegram bot
│   │   ├── discord.rs    # Discord bot
│   │   ├── slack.rs      # Slack integration
│   │   └── cli.rs        # Interactive CLI mode
│   ├── tools/
│   │   ├── mod.rs        # Tool trait & registry
│   │   ├── shell.rs      # Shell command execution (sandboxed)
│   │   ├── file.rs       # File operations
│   │   ├── memory.rs     # Memory storage/retrieval
│   │   ├── web.rs        # Web search & fetch
│   │   └── time.rs       # Scheduling & time utilities
│   ├── memory/
│   │   ├── mod.rs        # Memory backend trait
│   │   ├── sqlite.rs     # SQLite implementation
│   │   └── vector.rs     # Vector store for embeddings
│   ├── security/
│   │   ├── mod.rs        # Security primitives
│   │   ├── auth.rs       # User authentication/allowlist
│   │   └── sandbox.rs    # Execution sandboxing
│   └── utils/
│       ├── logger.rs     # Logging utilities
│       └── errors.rs     # Error types
├── skills/               # Skill definitions (YAML/JSON)
├── tests/                # Integration tests
├── Cargo.toml
└── README.md
```

## Component Specifications

### 1. Core Agent (`src/agent/`)

**Responsibilities:**
- Message intake and routing
- Context window management
- Tool call orchestration
- Response streaming

**Key Design:**
- Actor-based message processing
- Per-conversation context isolation
- Configurable context window (default: 4K tokens)

### 2. LLM Providers (`src/providers/`)

**Provider Trait:**
```rust
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;
    async fn stream(&self, request: CompletionRequest) -> Result<Stream<Chunk>>;
    fn supports_tools(&self) -> bool;
    fn max_context(&self) -> usize;
}
```

**Supported Providers:**
- OpenAI (GPT-4, GPT-3.5)
- Anthropic (Claude)
- Local models via Ollama/LM Studio
- DeepSeek, Moonshot, etc.

### 3. Channels (`src/channels/`)

**Channel Trait:**
```rust
pub trait Channel: Send + Sync {
    fn name(&self) -> &str;
    async fn start(&self, handler: MessageHandler) -> Result<()>;
    async fn send(&self, message: OutgoingMessage) -> Result<()>;
}
```

**Supported Channels:**
- Telegram (primary)
- Discord
- Slack
- CLI (interactive terminal)
- WebSocket (for custom integrations)

### 4. Tool System (`src/tools/`)

**Tool Trait:**
```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value; // JSON schema
    async fn execute(&self, args: Value, context: ToolContext) -> Result<ToolOutput>;
}
```

**Built-in Tools:**
| Tool | Description | Safety |
|------|-------------|--------|
| `shell` | Execute shell commands | Sandboxed, allowlist required |
| `file_read` | Read file contents | Path allowlist |
| `file_write` | Write files | Path allowlist, size limits |
| `memory_store` | Store information | Per-user isolation |
| `memory_search` | Retrieve memories | Own memories only |
| `web_fetch` | Fetch URL content | Domain allowlist |
| `web_search` | Search the web | Configurable provider |
| `time` | Get current time | Safe |
| `schedule` | Schedule reminders | Per-user limits |

### 5. Memory System (`src/memory/`)

**Features:**
- SQLite-based persistence
- Per-user/conversation isolation
- Vector embeddings for semantic search
- Automatic summarization for long contexts

**Schema:**
```sql
-- conversations
-- messages
-- memories (key-value + embedding vector)
-- users
-- allowlists
```

### 6. Security Model (`src/security/`)

**Layers:**
1. **Authentication**: Pairing codes for new users
2. **Authorization**: Explicit allowlist per channel
3. **Sandboxing**: Command execution in restricted environment
4. **Rate Limiting**: Per-user request throttling
5. **Input Validation**: Strict schema validation for all inputs

**Configuration:**
```yaml
security:
  pairing_required: true
  auto_allow_contacts: false
  sandbox:
    enabled: true
    allowed_commands: ["ls", "cat", "grep", "curl"]
    forbidden_paths: ["/etc/passwd", "~/.ssh"]
  rate_limits:
    requests_per_minute: 30
    tokens_per_day: 100000
```

### 7. Autonomy Features (Hermes-Agent Inspired)

Manta incorporates advanced autonomy mechanisms from Hermes-Agent to enable self-directed behavior and continuous improvement:

#### 7.1 Agent Loop with Iteration Budget

```rust
pub struct AgentLoop {
    max_iterations: usize,
    iteration_budget: Arc<AtomicUsize>,
    interrupt_flag: Arc<AtomicBool>,
}

impl AgentLoop {
    pub async fn run(&mut self, messages: Vec<Message>) -> Result<Response> {
        while self.iteration_budget.load(Ordering::Relaxed) > 0
              && !self.interrupt_flag.load(Ordering::Relaxed) {
            // Tool calling loop with budget tracking
        }
    }
}
```

- **Shared Budget**: Parent and child agents share a thread-safe iteration counter
- **Interrupt Mechanism**: Graceful shutdown via atomic flag
- **Cost Control**: Prevents runaway execution and excessive API costs

#### 7.2 Task Planning (Todo System)

```rust
pub struct TodoStore {
    tasks: Vec<Task>,
}

pub struct Task {
    id: String,
    content: String,
    status: TaskStatus, // pending, in_progress, completed, cancelled
    created_at: DateTime<Utc>,
}
```

- **Task Decomposition**: Break complex tasks into manageable steps
- **Status Tracking**: Real-time progress updates
- **Context Persistence**: Survives context window compression
- **Behavioral Guidance**: Tool schema encodes when to use ("Use for complex tasks with 3+ steps")

#### 7.3 Dual Memory Architecture

| Memory Type | Storage | Purpose | Access |
|-------------|---------|---------|--------|
| **Procedural Memory** | `~/.config/manta/memory/agent.md` | Environment facts, tool quirks, conventions | Agent R/W |
| **User Model** | `~/.config/manta/memory/user.md` | Preferences, communication style, habits | Agent R/W |
| **Ephemeral Memory** | SQLite | Session-specific context, temporary data | Tool-based |

**Features:**
- Bounded size (configurable, default: 4KB each)
- Frozen snapshots in system prompt (stable), live state via tool responses
- Security scanning for injection/exfiltration patterns

#### 7.4 Session Search

```rust
pub struct SessionSearch {
    db: SqlitePool,
}

impl SessionSearch {
    pub async fn search(&self, query: &str, user_id: &str) -> Result<Vec<SearchResult>> {
        // FTS5 full-text search across conversation history
        // LLM-based summarization of relevant sessions
    }
}
```

- **FTS5 Index**: Full-text search across all past conversations
- **Smart Summarization**: Auxiliary LLM compresses relevant sessions
- **Cross-Session Recall**: "When user references past conversation, use session_search"

#### 7.5 Autonomous Skill Creation

```rust
pub struct SkillManager;

impl SkillManager {
    pub async fn create_skill(&self, name: &str, content: SkillContent) -> Result<()>;
    pub async fn edit_skill(&self, name: &str, new_content: &str) -> Result<()>;
    pub async fn patch_skill(&self, name: &str, find: &str, replace: &str) -> Result<()>;
    pub async fn write_skill_file(&self, skill: &str, filename: &str, content: &str) -> Result<()>;
}
```

**Trigger**: After complex tasks (5+ tool calls) or discovering non-trivial workflows

**Skill Structure:**
```
skills/
└── {skill_name}/
    ├── SKILL.md          # Description, usage, examples
    ├── references/       # Supporting files
    └── scripts/          # Executable helpers
```

**Security**: Skills Guard scans for 50+ threat patterns before persistence

#### 7.6 Programmatic Tool Calling (PTC)

```rust
pub struct CodeExecutionTool {
    sandbox: Sandbox,
    rpc_socket: UnixSocket,
}
```

- **Self-Orchestration**: Agent writes Python scripts calling tools via RPC
- **Efficiency**: Collapses multi-step chains into single inference turn
- **Sandboxing**: 5-minute timeout, 50KB stdout limit, no network
- **Clean Abstraction**: Only script stdout returned to LLM

#### 7.7 Subagent Delegation

```rust
pub struct DelegateTool {
    max_children: usize,
    max_depth: usize,
}

impl DelegateTool {
    pub async fn spawn_child(&self, task: TaskSpec, parent_budget: Arc<AtomicUsize>)
        -> Result<ChildAgent>;
}
```

- **Parallel Execution**: Up to 3 concurrent child agents
- **Depth Limiting**: Max depth 2 (parent → child, no grandchildren)
- **Blocked Tools**: Children cannot use `delegate`, `clarify`, `memory`, `send_message`, `execute_code`
- **Progress Relay**: Child tool calls visible in parent UI

#### 7.8 Context Compression

```rust
pub struct ContextCompressor;

impl ContextCompressor {
    pub fn compress(&self, messages: Vec<Message>, target_tokens: usize) -> Vec<Message>;
}
```

- **Automatic Management**: Triggered when approaching context limit
- **Priority Preservation**: Todo list and critical context preserved
- **Cost Optimization**: Reduces token usage while maintaining coherence

#### 7.9 Scheduled Automation (Cron)

```rust
pub struct CronScheduler {
    jobs: Vec<ScheduledJob>,
}

pub struct ScheduledJob {
    schedule: CronSchedule,
    prompt: String,
    channel: ChannelId,
}
```

- **Natural Language Jobs**: Schedule tasks with natural language prompts
- **Multi-Platform Delivery**: Executes on Telegram, Discord, Slack, etc.
- **File-Based Locking**: Prevents concurrent execution
- **Output Mirroring**: Job results delivered to configured channels

#### 7.10 Persistent Assistant Spawning

Manta can create and manage other specialized Personal AI Assistants, each with their own identity, memory, and capabilities.

```rust
pub struct AssistantSpawner;

pub struct PersistentAssistant {
    pub id: String,
    pub name: String,
    pub specialization: AssistantType,
    pub system_prompt: String,
    pub memory: AssistantMemory,
    pub channels: Vec<ChannelConfig>,
    pub parent_id: Option<String>, // Manta's ID if spawned by Manta
}

pub enum AssistantType {
    Researcher,      // Deep research, analysis
    CodeReviewer,    // Code review, PR analysis
    Scheduler,       // Calendar, reminders, time management
    Social,          // Different persona/tone for social channels
    Specialist(String), // Custom specialization
}

impl AssistantSpawner {
    /// Spawn a new persistent assistant
    pub async fn spawn(
        &self,
        config: AssistantConfig,
    ) -> Result<PersistentAssistant>;

    /// List all managed assistants
    pub async fn list_assistants(&self) -> Vec<PersistentAssistant>;

    /// Send message to a specific assistant
    pub async fn message_assistant(
        &self,
        assistant_id: &str,
        message: &str,
    ) -> Result<String>;

    /// Terminate an assistant
    pub async fn terminate(&self, assistant_id: &str) -> Result<()>;
}
```

**Why Spawn Separate Assistants?**

| Reason | Benefit |
|--------|---------|
| **Isolation** | One assistant crashing doesn't affect others |
| **Specialization** | Different system prompts, tools, memory per role |
| **Resource Management** | Limit resources per assistant |
| **Privacy** | Sensitive data isolated to specific assistants |
| **Scaling** | Distribute load across multiple instances |

**Assistant Lifecycle:**

```
1. Manta decides to spawn specialized assistant
   ↓
2. Generate configuration (name, specialization, channels)
   ↓
3. Create isolated environment (separate DB, memory, config)
   ↓
4. Start assistant process/container
   ↓
5. Monitor and manage (restart if crashed, update config)
   ↓
6. Can terminate or upgrade independently
```

**Communication Between Assistants:**

```rust
pub struct AssistantMesh;

impl AssistantMesh {
    /// Route message between assistants
    pub async fn route(
        &self,
        from: &str,
        to: &str,
        message: &str,
    ) -> Result<String>;

    /// Broadcast to all assistants
    pub async fn broadcast(&self, message: &str) -> Vec<Result<String>>;
}
```

**Example Use Cases:**

```rust
// Spawn a research assistant
let researcher = spawner.spawn(AssistantConfig {
    name: "ResearchBot".to_string(),
    specialization: AssistantType::Researcher,
    channels: vec![ChannelConfig::telegram("@research_bot")],
    system_prompt: "You are a deep research assistant...".to_string(),
}).await?;

// Spawn a code review assistant
let code_reviewer = spawner.spawn(AssistantConfig {
    name: "CodeReviewBot".to_string(),
    specialization: AssistantType::CodeReviewer,
    channels: vec![ChannelConfig::slack("#code-reviews")],
    tools: vec!["github", "linter", "test_runner"],
}).await?;

// Route work between assistants
let research = mesh.message_assistant(
    &researcher.id,
    "Research Rust async patterns"
).await?;

let review = mesh.message_assistant(
    &code_reviewer.id,
    &format!("Review this code that uses async: {}", research)
).await?;
```

**Spawned Assistant Architecture:**

Each spawned assistant has:
- **Isolated SQLite database** for conversations and memory
- **Separate configuration** (can override parent's settings)
- **Filtered tool access** (parent can restrict available tools)
- **Shared parent API keys** (or can have its own)
- **Independent channels** (but can share with parent)

**Security:**
- Spawned assistants run in separate processes/containers
- Parent can monitor and terminate children
- Children cannot spawn further assistants (prevent recursion)
- Resource quotas per assistant

#### 7.11 Autonomy Architecture Updates

Updated directory structure with autonomy components:

```
manta/
├── src/
│   ├── agent/
│   │   ├── mod.rs              # Core agent orchestration
│   │   ├── context.rs          # Conversation context management
│   │   ├── router.rs           # Request routing & handling
│   │   ├── loop.rs             # Autonomous agent loop with budget
│   │   ├── todo.rs             # Task planning system
│   │   ├── compressor.rs       # Context compression
│   │   └── delegate.rs         # Subagent spawning
│   ├── memory/
│   │   ├── mod.rs              # Memory backend trait
│   │   ├── sqlite.rs           # SQLite implementation
│   │   ├── vector.rs           # Vector store for embeddings
│   │   ├── dual.rs             # Dual memory (procedural + user model)
│   │   └── session_search.rs   # FTS5 session search
│   ├── skills/
│   │   ├── mod.rs              # Skill manager
│   │   ├── guard.rs            # Security scanning
│   │   └── loader.rs           # Skill loading & execution
│   ├── tools/
│   │   ├── mod.rs              # Tool trait & registry
│   │   ├── shell.rs            # Shell command execution
│   │   ├── file.rs             # File operations
│   │   ├── memory.rs           # Memory storage/retrieval
│   │   ├── web.rs              # Web search & fetch
│   │   ├── time.rs             # Scheduling & time utilities
│   │   ├── todo_tool.rs        # Task management tool
│   │   ├── code_exec.rs        # PTC code execution
│   │   ├── delegate_tool.rs    # Subagent delegation
│   │   ├── session_search.rs   # Session search tool
│   │   └── mcp.rs              # MCP client integration
│   ├── assistants/             # Persistent assistant spawning
│   │   ├── mod.rs              # Assistant spawner
│   │   ├── mesh.rs             # Inter-assistant communication
│   │   └── monitor.rs          # Health monitoring
│   └── cron/
│       ├── mod.rs              # Cron scheduler
│       └── jobs.rs             # Job definitions
├── assistants/                 # Spawned assistant instances
│   └── {assistant_id}/
│       ├── config.yaml
│       └── state.db
├── skills/                     # User and agent-created skills
│   └── {skill_name}/
│       ├── SKILL.md
│       └── ...
└── memory/
    ├── agent.md                # Procedural memory
    └── user.md                 # User model
```

## Implementation Phases

### Phase 1: Foundation (Week 1-2)
- [ ] Project setup (Cargo.toml, workspace structure)
- [ ] Error handling and logging
- [ ] Configuration system
- [ ] Basic trait definitions (Provider, Channel, Tool)

### Phase 2: Core Agent (Week 2-3)
- [ ] Agent orchestration loop
- [ ] Context management
- [ ] OpenAI provider implementation
- [ ] CLI channel for testing

### Phase 3: Tools & Memory (Week 3-4)
- [ ] Tool registry and execution
- [ ] Shell tool with sandboxing
- [ ] File tools with allowlists
- [ ] SQLite memory backend

### Phase 4: Channels (Week 4-5)
- [ ] Telegram channel
- [ ] Discord channel
- [ ] Message formatting

### Phase 5: Security (Week 5-6)
- [ ] Authentication system
- [ ] Allowlist management
- [ ] Rate limiting
- [ ] Security audit

### Phase 6: Autonomy Features (Week 6-8)
- [ ] Agent loop with iteration budget
- [ ] Task planning (todo system)
- [ ] Dual memory architecture
- [ ] Session search with FTS5
- [ ] Context compression
- [ ] Cron scheduler

### Phase 7: Advanced Autonomy (Week 8-10)
- [ ] Autonomous skill creation
- [ ] Skills Guard security scanning
- [ ] Programmatic Tool Calling (PTC)
- [ ] Subagent delegation
- [ ] Persistent assistant spawning
- [ ] Assistant mesh communication
- [ ] Skill hub / sharing

### Phase 8: Polish (Week 10-11)
- [ ] Documentation
- [ ] Example skills
- [ ] Deployment configs
- [ ] Performance optimization
- [ ] Security audit

## Technical Specifications

### Performance Targets
- Binary size: <10MB (stripped, release)
- Memory usage: <20MB baseline
- Startup time: <50ms
- Request latency: <100ms (excluding LLM)

### Dependencies (Minimal)
```toml
[dependencies]
# Async runtime
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# HTTP client
reqwest = { version = "0.11", features = ["json", "stream"] }

# Database
sqlx = { version = "0.7", features = ["sqlite", "runtime-tokio"] }

# Configuration
config = "0.14"

# Logging
tracing = "0.1"
tracing-subscriber = "0.3"

# CLI
clap = { version = "4", features = ["derive"] }

# Error handling
thiserror = "1"
anyhow = "1"
```

### Feature Flags
```toml
[features]
default = ["telegram", "sqlite"]
all = ["telegram", "discord", "slack", "sqlite", "vector"]
telegram = ["teloxide"]
discord = ["serenity"]
slack = ["slack-morphism"]
vector = ["pgvector", "fastembed"]
local-llm = ["ollama-rs"]
```

## Configuration

### File: `~/.config/manta/config.yaml`
```yaml
# LLM Configuration
provider:
  type: openai
  api_key: "${OPENAI_API_KEY}"
  model: gpt-4o-mini
  temperature: 0.7

# Bot Personality
agent:
  name: "Manta"
  system_prompt: |
    You are Manta, a helpful personal AI assistant.
    You have access to tools for file operations, web search,
    and memory. Always be concise and helpful.

# Active Channels
channels:
  telegram:
    enabled: true
    token: "${TELEGRAM_BOT_TOKEN}"
  discord:
    enabled: false
    token: "${DISCORD_BOT_TOKEN}"
  cli:
    enabled: true

# Security
security:
  admin_users: ["@telegram_username"]
  allow_by_default: false

# Storage
data_dir: "~/.local/share/manta"
```

## Skills System

Skills extend Manta's capabilities through declarative definitions:

```yaml
# skills/weather.yaml
name: weather
description: Get weather information
tools:
  - web_fetch
triggers:
  - regex: "weather in (.+)"
  - intent: "check_weather"
prompt: |
  When asked about weather, use web_fetch to get data from
  wttr.in/{location}?format=3
```

## Deployment

### Local Development
```bash
cargo run -- --config dev.yaml
```

### Production Build
```bash
cargo build --release --features all
strip target/release/manta
```

### Docker
```dockerfile
FROM scratch
COPY target/release/manta /manta
COPY config.yaml /config.yaml
ENTRYPOINT ["/manta", "--config", "/config.yaml"]
```

### Systemd Service
```ini
[Unit]
Description=Manta AI Assistant
After=network.target

[Service]
Type=simple
User=manta
ExecStart=/usr/local/bin/manta
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

## Testing Strategy

- **Unit Tests**: Core logic, trait implementations
- **Integration Tests**: End-to-end with mock providers
- **Channel Tests**: Test each channel separately
- **Security Tests**: Fuzzing, penetration testing

## Future Enhancements

1. **Voice Support**: Whisper for STT, TTS integration
2. **Vision**: Image understanding capabilities
3. **Multi-Agent**: Swarm coordination patterns
4. **Hardware**: GPIO support for Raspberry Pi
5. **Plugins**: WASM-based plugin system
6. **RL Training**: Integration with Atropos for training tool-calling models
7. **Embeddings**: Local embedding models for semantic search
8. **Knowledge Base**: RAG with document ingestion

## References

- **NanoClaw**: Container-first TypeScript implementation (~4K lines)
- **ZeroClaw**: Rust implementation with trait-driven architecture (<5MB RAM)
- **Hermes-Agent**: Python-based autonomous agent with closed learning loop (40+ tools, skill creation, RL training)
- **Model Context Protocol**: Anthropic's tool use standard
- **Atropos**: Nous Research RL training framework for tool-calling models
