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

### Phase 6: Polish (Week 6-7)
- [ ] Documentation
- [ ] Example skills
- [ ] Deployment configs
- [ ] Performance optimization

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

## References

- NanoClaw: Container-first TypeScript implementation
- ZeroClaw: Rust implementation with trait-driven architecture
- Model Context Protocol: Anthropic's tool use standard
