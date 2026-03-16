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
| **Procedural Memory** | `~/.manta/memory/agent.md` | Environment facts, tool quirks, conventions | Agent R/W |
| **User Model** | `~/.manta/memory/user.md` | Preferences, communication style, habits | Agent R/W |
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

#### 1.1 Project Setup
- [✅] Initialize Cargo workspace with `Cargo.toml`
- [✅] Create workspace structure: `src/`, `tests/`, `examples/`
- [✅] Set up Rust edition 2021 and MSRV (1.75+)
- [✅] Configure `.gitignore` for Rust project
- [✅] Create `rustfmt.toml` and `clippy.toml`

#### 1.2 Error Handling
- [✅] Define `Error` enum with `thiserror`
- [✅] Create `Result<T>` type alias
- [✅] Implement error conversion traits
- [✅] Add error context with `anyhow`
- [✅] Create error response formatter for users

#### 1.3 Logging
- [✅] Set up `tracing` subscriber
- [✅] Configure log levels (ERROR, WARN, INFO, DEBUG, TRACE)
- [✅] Add structured logging with `tracing-subscriber`
- [✅] Create log rotation for production
- [✅] Add span tracing for async operations

#### 1.4 Configuration System
- [✅] Define `Config` struct with `serde`
- [✅] Support YAML/JSON/TOML formats
- [✅] Implement environment variable interpolation (`${VAR}`)
- [✅] Add config validation on load
- [✅] Create config hot-reload mechanism
- [✅] Write `config.example.yaml`

#### 1.5 Core Traits
- [✅] Define `Provider` trait with async methods
- [✅] Define `Channel` trait with message handlers
- [✅] Define `Tool` trait with JSON schema
- [✅] Create `Message` struct for conversations
- [✅] Define `ToolCall` and `ToolResult` types

### Phase 2: Core Agent (Week 2-3)

#### 2.1 Message Loop
- [✅] Create `Agent` struct
- [✅] Implement message intake queue
- [✅] Add message dispatcher
- [✅] Create response formatter
- [✅] Add concurrent request handling
- [✅] Implement graceful shutdown

#### 2.2 Context Management
- [✅] Define `Context` struct for conversations
- [✅] Implement message history storage
- [✅] Add context window tracking (token counting)
- [✅] Create context pruning strategy
- [✅] Implement system prompt injection
- [✅] Add per-user context isolation

#### 2.3 Provider Abstraction
- [✅] Create `ProviderRegistry`
- [✅] Implement OpenAI provider
  - [✅] Chat completions API
  - [✅] Streaming responses
  - [✅] Tool calling support
  - [✅] Error handling for API failures
- [✅] Add provider fallback mechanism
- [✅] Implement request/response logging

#### 2.4 Tool Orchestration
- [✅] Create `ToolRegistry` with tool discovery
- [✅] Implement tool schema generation
- [✅] Add tool call parsing from LLM responses
- [✅] Create tool result formatter
- [✅] Implement parallel tool execution
- [✅] Add tool execution timeouts

#### 2.5 CLI Channel ✅
- [✅] Set up `clap` CLI parser
- [✅] Implement interactive REPL mode
- [✅] Add command history (readline)
- [✅] Create rich output formatting
- [✅] Add `exit`, `clear`, `help`, `tools` commands
- [✅] Support single message mode (-m flag)
- [✅] Provider-agnostic configuration (MANTA_BASE_URL, MANTA_API_KEY, MANTA_MODEL)

### Phase 3: Tools & Memory (Week 3-4)

#### 3.1 Tool Registry
- [✅] Create `ToolRegistrar` for dynamic tools
- [✅] Implement tool name validation
- [✅] Add tool description generation
- [✅] Create tool parameter validation
- [✅] Implement tool result caching

#### 3.2 Shell Tool
- [✅] Define `ShellTool` struct
- [✅] Implement command allowlist
- [✅] Add command timeout (30s default)
- [✅] Capture stdout/stderr
- [✅] Implement working directory restrictions
- [✅] Add environment variable filtering
- [✅] Create output truncation (max 10KB)

#### 3.3 File Tools
- [✅] Implement `FileReadTool`
  - [✅] Path allowlist validation
  - [✅] File size limits (1MB)
  - [✅] Binary file detection
- [✅] Implement `FileWriteTool`
  - [✅] Directory creation
  - [✅] Backup existing files
  - [✅] Size limits
- [✅] Implement `FileEditTool`
  - [✅] Find/replace with regex
  - [✅] Line-based edits
  - [✅] Atomic writes
- [✅] Implement `GlobTool`
  - [✅] Pattern matching
  - [✅] Result limits (100 files)
- [✅] Implement `GrepTool`
  - [✅] Regex search
  - [✅] Context lines
  - [✅] Result limits

#### 3.4 Memory System ✅
- [✅] Create `MemoryStore` trait
- [✅] Implement SQLite backend
  - [✅] Schema migrations
  - [✅] Connection pooling
- [✅] Create `Memory` struct with embeddings
- [✅] Implement memory storage
- [✅] Add memory retrieval by ID
- [✅] Implement semantic search
- [✅] Add memory expiration/TTL

#### 3.5 Web Tools ✅
- [✅] Implement `WebFetchTool`
  - [✅] HTTP GET requests
  - [✅] Content type detection
  - [✅] HTML to markdown conversion
  - [✅] Size limits (100KB)
- [✅] Implement `WebSearchTool`
  - [✅] Search provider abstraction
  - [✅] Result formatting
  - [✅] Rate limiting

### Phase 4: Channels (Week 4-5)

#### 4.1 Telegram Channel [✅]
- [✅] Set up `teloxide` dependency
- [✅] Implement bot authentication
- [✅] Handle `/start` command
- [✅] Implement message receiving
- [✅] Add message sending with formatting
- [✅] Handle message edits
- [✅] Add file/photo support
- [✅] Implement typing indicators

#### 4.2 Discord Channel [✅]
- [✅] Set up `serenity` dependency
- [✅] Implement bot authentication
- [✅] Handle DM messages
- [✅] Handle guild/channel messages
- [✅] Add message sending
- [✅] Implement embed support
- [✅] Add slash command registration
- [✅] Handle message reactions

#### 4.3 Slack Channel [✅]
- [✅] Set up Web API integration
- [✅] Implement bot authentication
- [✅] Handle app mentions
- [✅] Handle DM messages
- [✅] Add message posting
- [✅] Implement block kit formatting
- [✅] Handle file shares

#### 4.4 Message Formatting [✅]
- [✅] Create `MessageFormatter` trait
- [✅] Implement Markdown to Telegram HTML
- [✅] Implement Markdown to Discord markdown
- [✅] Implement Markdown to Slack mrkdwn
- [✅] Add code block formatting
- [✅] Handle mentions/usernames

### Phase 5: Security (Week 5-6)

#### 5.1 Authentication
- [✅] Create `AuthManager`
- [✅] Implement pairing code generation
- [✅] Add user registration flow
- [✅] Store user credentials securely
- [✅] Implement session management
- [✅] Add device fingerprinting

#### 5.2 Allowlist Management
- [✅] Create `Allowlist` struct
- [✅] Implement user ID validation
- [✅] Add channel-specific allowlists
- [✅] Implement admin override
- [✅] Add temporary access grants
- [✅] Create allowlist persistence

#### 5.3 Rate Limiting
- [✅] Implement token bucket algorithm
- [✅] Add per-user rate limits
- [✅] Add per-channel rate limits
- [✅] Implement global rate limits
- [✅] Add rate limit headers
- [✅] Create rate limit notifications

#### 5.4 Input Validation
- [✅] Add JSON schema validation
- [✅] Implement path traversal detection
- [✅] Add command injection detection
- [✅] Sanitize user inputs
- [✅] Validate message lengths

#### 5.5 Security Audit
- [✅] Run `cargo audit`
- [✅] Review dependency tree
- [✅] Check for unsafe code usage
- [✅] Implement secret scanning
- [✅] Add security headers
- [✅] Document security model

### Phase 6: Autonomy Features (Week 6-8)

#### 6.1 Iteration Budget
- [✅] Create `IterationBudget` struct
- [✅] Implement atomic counter
- [✅] Add budget sharing between parent/child
- [✅] Create budget exhaustion handlers
- [✅] Add budget configuration
- [✅] Implement budget warnings

#### 6.2 Task Planning (Todo)
- [✅] Define `Task` struct
- [✅] Create `TodoStore` in-memory
- [✅] Implement task CRUD operations
- [✅] Add task status tracking
- [✅] Implement task dependencies
- [✅] Create task persistence
- [✅] Add task notifications

#### 6.3 Dual Memory
- [✅] Create `ProceduralMemory` (agent.md)
- [✅] Create `UserModel` (user.md)
- [✅] Implement memory file reading
- [✅] Add memory file writing
- [✅] Implement memory size limits (4KB)
- [✅] Add memory injection to prompts

#### 6.4 Session Search
- [✅] Set up SQLite FTS5
- [✅] Index conversation messages
- [✅] Implement full-text search
- [✅] Add result ranking
- [✅] Implement LLM summarization
- [✅] Add search result formatting

#### 6.5 Context Compression
- [✅] Create `ContextCompressor`
- [✅] Implement token counting
- [✅] Add message summarization
- [✅] Create priority scoring
- [✅] Implement sliding window
- [✅] Add compression triggers

#### 6.6 Cron Scheduler
- [✅] Create `CronScheduler` struct
- [✅] Parse cron expressions
- [✅] Implement job queue
- [✅] Add job execution
- [✅] Implement job persistence
- [✅] Add job notifications
- [✅] Handle missed executions

### Phase 7: Advanced Autonomy (Week 8-10)

#### 7.1 Autonomous Skill Creation
- [✅] Create `SkillManager`
  - [✅] Define skill structure
  - [✅] Create skill validation
  - [✅] Implement skill storage
- [✅] Implement skill generation
  - [✅] Prompt for skill creation
  - [✅] Generate SKILL.md
  - [✅] Create supporting files
- [✅] Add skill loading
- [✅] Implement skill execution
- [✅] Create skill versioning

#### 7.2 Skills Guard
- [✅] Define security patterns (50+)
- [✅] Implement pattern matching
- [✅] Add code injection detection
- [✅] Detect secret exfiltration
- [✅] Check for privilege escalation
- [✅] Create security report

#### 7.3 Programmatic Tool Calling (PTC)
- [✅] Create `CodeExecutionTool`
- [✅] Set up Python sandbox
- [✅] Implement RPC between Python and agent
- [✅] Add tool call serialization
- [✅] Implement result capture
- [✅] Add execution timeout
- [✅] Create output size limits

#### 7.4 Subagent Delegation
- [✅] Create `DelegateTool`
- [✅] Implement child agent spawning
- [✅] Add parent-child communication
- [✅] Implement budget sharing
- [✅] Add depth limiting (max 2)
- [✅] Create child isolation
- [✅] Implement result aggregation

#### 7.5 Persistent Assistant Spawning
- [✅] Create `AssistantSpawner`
- [✅] Define `AssistantType` enum
- [✅] Implement assistant configuration
- [✅] Create isolated environment per assistant
- [✅] Implement assistant lifecycle
- [✅] Add `AssistantMesh` for communication
- [✅] Create assistant monitoring
- [✅] Implement resource quotas

#### 7.6 Assistant Mesh
- [✅] Implement message routing
- [✅] Add broadcast capability
- [✅] Create discovery mechanism
- [✅] Implement load balancing
- [✅] Add failure detection
- [✅] Create mesh topology

#### 7.7 MCP Integration
- [✅] Create `McpClient`
- [✅] Implement stdio transport
- [✅] Implement SSE transport
- [✅] Add tool discovery
- [✅] Implement MCP tool calling
- [✅] Add server management

### Phase 8: Polish (Week 10-11)

#### 8.1 Documentation
- [✅] Write API documentation
- [✅] Create architecture diagrams
- [✅] Write user guide
- [✅] Add deployment guide
- [✅] Create troubleshooting guide
- [✅] Write contribution guidelines
- [✅] Add changelog

#### 8.2 Example Skills
- [✅] Create weather skill
- [✅] Create news skill
- [✅] Create todo management skill
- [✅] Create calculator skill
- [✅] Create reminder skill
- [✅] Add skill templates

#### 8.3 Deployment
- [✅] Create Dockerfile
- [✅] Write docker-compose.yml
- [✅] Create systemd service file
- [✅] Write Kubernetes manifests
- [✅] Add GitHub Actions CI/CD
- [✅] Create release script
- [✅] Write installation guide

#### 8.4 Performance Optimization
- [✅] Profile CPU usage
- [✅] Optimize memory allocations
- [✅] Add connection pooling
- [✅] Implement caching
- [✅] Optimize database queries
- [✅] Add request batching
- [✅] Create benchmarks

#### 8.5 Final Security Audit
- [✅] Review all permissions
- [✅] Audit tool implementations
- [✅] Check for data leaks
- [✅] Verify sandboxing
- [✅] Run penetration tests
- [✅] Document security boundaries

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

### File: `~/.manta/config.yaml`
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
