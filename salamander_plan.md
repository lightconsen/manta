# Salamander - Self-Managing Personal AI Assistant

## Vision

Salamander is a **self-managing**, lightweight, fast, and secure Personal AI Assistant written in Rust. It introduces a revolutionary **Kernel + Mutable Runtime** architecture that enables the agent to write, compile, and reload its own behavior code safely.

**Key Innovation**: An immutable Rust "Kernel" provides a sandboxed WASM runtime where the agent's "behavior layer" executes. The agent can generate, compile, and hot-reload new WASM modules to extend its capabilities, fix bugs, and optimize performance—all while the Kernel ensures safety through resource limits and capability-based sandboxing.

**Target**: <15MB binary size (including WASM runtime), <30MB RAM usage.

## Core Principles

1. **Self-Managing**: The agent can write, compile, and reload its own behavior code
2. **Immutable Kernel**: Core runtime is fixed; all behavior lives in sandboxed WASM
3. **Lightweight**: Minimal resource footprint, fast startup
4. **Secure by Design**: Deny-by-default, explicit allowlists, sandboxed execution
5. **AI-Native**: No dashboards, natural language interface
6. **Single Binary**: Easy deployment, minimal dependencies

## Architecture

### Kernel + Mutable Runtime Pattern

```
┌─────────────────────────────────────────────────────────────────────┐
│  KERNEL (Immutable Rust Binary)                                     │
│  ├─ Main Loop          - Message intake, lifecycle management       │
│  ├─ WASM Runtime       - wasmtime sandboxed execution               │
│  ├─ LLM Client         - Provider abstraction (fixed)               │
│  ├─ Resource Limits    - Fuel meter, memory limits, timeouts        │
│  ├─ State Persistence  - SQLite, versioned storage                  │
│  ├─ Hot Reload         - Atomic runtime swaps                       │
│  └─ Security Policy    - Capability-based sandboxing                │
├─────────────────────────────────────────────────────────────────────┤
│  MUTABLE RUNTIME (WASM - Self-Coded)                                │
│  ├─ Agent Behavior     - Decision logic, tool orchestration         │
│  ├─ Tool Implementations - Custom tools written by agent            │
│  ├─ Memory Management  - Vector search, session handling            │
│  ├─ Channel Handlers   - Protocol-specific message handling         │
│  ├─ Self-Improvement   - Code generation, optimization loops        │
│  └─ Skills System      - Dynamic capability modules                 │
└─────────────────────────────────────────────────────────────────────┘
```

### Directory Structure

```
salamander/
├── src/
│   ├── main.rs              # CLI entry point
│   ├── kernel/              # IMMUTABLE CORE
│   │   ├── mod.rs           # Kernel orchestration
│   │   ├── runtime.rs       # WASM runtime management
│   │   ├── reload.rs        # Hot reload mechanism
│   │   ├── limits.rs        # Resource limits (fuel, memory, time)
│   │   ├── llm.rs           # LLM client (fixed)
│   │   └── state.rs         # Persistent state management
│   ├── runtime/             # Mutable layer (loaded into WASM)
│   │   ├── lib.rs           # Runtime entry point (WASM)
│   │   ├── agent.rs         # Agent behavior logic
│   │   ├── tools.rs         # Tool definitions
│   │   ├── memory.rs        # Memory management
│   │   └── channels.rs      # Channel implementations
│   ├── providers/
│   │   ├── mod.rs           # Provider trait
│   │   ├── openai.rs
│   │   ├── anthropic.rs
│   │   └── local.rs
│   └── codegen/             # Self-coding support
│       ├── mod.rs           # Code generation orchestration
│       ├── compiler.rs      # Rust → WASM compilation
│       ├── validator.rs     # Safety checks before reload
│       └── version.rs       # Version control for runtimes
├── runtime/                 # Generated WASM runtimes (versioned)
│   ├── v1.0.0.wasm
│   ├── v1.0.1.wasm
│   └── current -> v1.0.1.wasm
├── state/                   # Persistent state
│   ├── conversations.db
│   ├── memories/
│   └── runtime_state.json
└── Cargo.toml
```

## Self-Management System (Core Feature)

Salamander's defining feature is its ability to write, compile, and reload its own behavior code safely. This is achieved through the **Kernel + Mutable Runtime** architecture.

### 1. The Immutable Kernel

The Kernel is a fixed Rust binary that provides:

```rust
// src/kernel/mod.rs
pub struct Kernel {
    wasm_engine: Engine,                    // wasmtime instance
    runtime: Option<WasmRuntime>,           // currently loaded mutable layer
    llm_client: LlmClient,                  // fixed LLM client
    state: PersistentState,                 // SQLite + file storage
    limits: ResourceLimits,                 // fuel, memory, time
}

impl Kernel {
    /// Execute message through WASM runtime
    pub async fn execute(&mut self, input: &str) -> Result<String> {
        let runtime = self.runtime.as_ref()
            .ok_or_else(|| Error::NoRuntime)?;

        // Call with resource limits
        runtime.call_with_limits(
            "handle_message",
            input,
            self.limits.clone()
        ).await
    }

    /// Hot reload the mutable runtime
    pub async fn reload(&mut self, wasm_bytes: Vec<u8>) -> Result<()> {
        // 1. Save current state
        let state = self.runtime.as_ref().map(|r| r.save_state());

        // 2. Compile and validate new runtime
        let new_runtime = WasmRuntime::new(&self.wasm_engine, wasm_bytes)?;

        // 3. Atomic swap
        let old_runtime = self.runtime.replace(new_runtime);

        // 4. Restore state (if compatible)
        if let (Some(s), Some(r)) = (state, self.runtime.as_mut()) {
            r.restore_state(s)?;
        }

        // 5. Persist new runtime to disk
        self.persist_runtime().await?;

        Ok(())
    }
}
```

**Kernel Guarantees:**
- Cannot be modified by the agent
- Enforces resource limits (fuel metering, memory caps)
- Handles all I/O (network, filesystem)
- Manages persistent state
- Provides LLM API access to WASM runtime

### 2. The Mutable Runtime (WASM)

The Runtime is a WASM module containing the agent's behavior:

```rust
// src/runtime/lib.rs - compiled to WASM
#[no_mangle]
pub extern "C" fn handle_message(input_ptr: *const u8, input_len: usize) -> *mut u8 {
    let input = unsafe {
        std::slice::from_raw_parts(input_ptr, input_len)
    };
    let message = std::str::from_utf8(input).unwrap();

    // Agent behavior - can be self-modified
    let response = AGENT_BEHAVIOR.handle(message);

    // Return to kernel
    serialize_response(response)
}

static mut AGENT_BEHAVIOR: AgentBehavior = AgentBehavior::new();

pub struct AgentBehavior {
    tools: ToolRegistry,
    memory: MemoryManager,
    channels: ChannelManager,
}

impl AgentBehavior {
    pub fn handle(&self, message: &str) -> Response {
        // This logic can be rewritten by the agent itself
        // through code generation
    }
}
```

**Runtime Capabilities:**
- Define tools, channels, memory strategies
- Self-modify behavior through code generation
- Access kernel-provided host functions
- Cannot escape sandbox (no direct I/O)

### 3. Host Functions (Kernel → Runtime Interface)

The WASM runtime can call these kernel-provided functions:

```rust
// Host function interface (WASI-like)
#[host_function]
pub fn llm_complete(prompt: &str) -> String;

#[host_function]
pub fn llm_stream(prompt: &str, callback: fn(&str));

#[host_function]
pub fn storage_get(key: &str) -> Option<Vec<u8>>;

#[host_function]
pub fn storage_set(key: &str, value: &[u8]);

#[host_function]
pub fn http_request(method: &str, url: &str, body: Option<&str>) -> HttpResponse;

#[host_function]
pub fn sandbox_exec(command: &str, args: &[&str]) -> ExecResult;

#[host_function]
pub fn log(level: LogLevel, message: &str);

// Critical: Code generation function
#[host_function]
pub fn generate_and_reload(prompt: &str) -> Result<(), String>;
```

### 4. The Self-Coding Loop

```rust
// Self-management workflow
pub async fn self_improve(kernel: &mut Kernel, goal: &str) -> Result<()> {
    // 1. Analyze current runtime
    let current_code = kernel.runtime.get_source_code();
    let current_metrics = kernel.runtime.profile_performance();

    // 2. Generate improved code via LLM
    let improvement_prompt = format!(r#"
Current code:
{}

Performance metrics:
{:?}

Goal: {}

Generate improved Rust code for the WASM runtime.
The code must:
1. Compile to wasm32-unknown-unknown target
2. Export 'handle_message' function
3. Use only kernel host functions for I/O
4. Be memory-safe and efficient

Return ONLY the code, no explanation.
"#, current_code, current_metrics, goal);

    let new_code = kernel.llm.complete(&improvement_prompt).await?;

    // 3. Compile to WASM
    let wasm_bytes = kernel.codegen.compile(&new_code).await?;

    // 4. Validate safety
    kernel.codegen.validate(&wasm_bytes)?;

    // 5. Test in shadow environment
    let test_result = kernel.test_runtime(&wasm_bytes).await?;

    // 6. If tests pass, hot reload
    if test_result.success {
        kernel.reload(wasm_bytes).await?;
        kernel.state.save_version(&new_code, &test_result).await?;
    }

    Ok(())
}
```

### 5. Resource Limits & Safety

```rust
pub struct ResourceLimits {
    // Execution limits
    pub max_fuel: u64,              // WASM instructions (e.g., 10 billion)
    pub max_memory: usize,          // WASM memory (e.g., 128MB)
    pub max_execution_time: Duration, // Wall clock time (e.g., 30s)

    // Code generation limits
    pub max_code_size: usize,       // Generated source size (e.g., 1MB)
    pub max_wasm_size: usize,       // Compiled WASM size (e.g., 10MB)
    pub daily_reloads: usize,       // Max reloads per day (e.g., 100)

    // API limits
    pub max_llm_calls: usize,       // Per execution
    pub max_storage_ops: usize,     // Per execution
}

impl ResourceLimits {
    pub fn default() -> Self {
        Self {
            max_fuel: 10_000_000_000,  // ~10B instructions
            max_memory: 128 * 1024 * 1024, // 128MB
            max_execution_time: Duration::from_secs(30),
            max_code_size: 1024 * 1024, // 1MB source
            max_wasm_size: 10 * 1024 * 1024, // 10MB WASM
            daily_reloads: 100,
            max_llm_calls: 10,
            max_storage_ops: 1000,
        }
    }
}
```

### 6. Version Control & Rollback

```rust
pub struct RuntimeVersion {
    pub version: semver::Version,
    pub wasm_hash: String,          // SHA256 of WASM
    pub source_code: String,        // Original Rust source
    pub created_at: DateTime<Utc>,
    pub performance_metrics: Metrics,
    pub test_results: TestResults,
    pub rollback_reason: Option<String>,
}

pub struct VersionManager {
    versions: Vec<RuntimeVersion>,
    current: usize,
}

impl VersionManager {
    /// Rollback to previous version if current fails
    pub async fn rollback(&mut self, kernel: &mut Kernel) -> Result<()> {
        if self.current == 0 {
            return Err(Error::CannotRollbackFurther);
        }

        self.current -= 1;
        let version = &self.versions[self.current];

        // Load WASM from disk
        let wasm_bytes = fs::read(format!("runtime/{}.wasm", version.version)).await?;

        kernel.reload(wasm_bytes).await?;

        Ok(())
    }

    /// Auto-rollback on panic/timeout
    pub async fn auto_rollback(&mut self, kernel: &mut Kernel, error: &Error) -> Result<()> {
        version.rollback_reason = Some(error.to_string());
        self.rollback(kernel).await
    }
}
```

### 7. Self-Healing on Errors

```rust
impl Kernel {
    pub async fn execute_with_healing(&mut self, input: &str) -> Result<String> {
        match self.execute(input).await {
            Ok(result) => Ok(result),
            Err(e) => {
                log::error!("Runtime error: {}", e);

                // Attempt self-healing
                let healing_prompt = format!(r#"
The runtime encountered an error:
{}

Current source code:
{}

Fix the bug and return corrected code.
"#, e, self.runtime.get_source_code());

                let fixed_code = self.llm.complete(&healing_prompt).await?;
                let wasm = self.codegen.compile(&fixed_code).await?;

                self.reload(wasm).await?;

                // Retry once
                self.execute(input).await
            }
        }
    }
}
```

### 8. Capabilities

The agent can self-modify to add:

| Capability | Description | Example |
|------------|-------------|---------|
| **Custom Tools** | Write new tool implementations | "Add a tool to parse CSV files" |
| **Channel Support** | Add new messaging channels | "Add Matrix protocol support" |
| **Memory Strategies** | Implement new memory backends | "Add Redis backend for distributed memory" |
| **Optimization** | Improve performance | "Optimize the tool dispatch logic" |
| **Bug Fixes** | Self-heal errors | Automatically fix panics |
| **Protocol Handlers** | Add new communication protocols | "Add WebSocket server capability" |

### 9. Security Boundaries

**What the Runtime CANNOT do:**
- Access filesystem directly (only through kernel host functions)
- Make network calls directly (only through kernel HTTP client)
- Access environment variables
- Spawn processes
- Access kernel memory
- Modify resource limits
- Bypass fuel metering

**What the Runtime CAN do:**
- Define its own logic
- Request kernel to perform I/O
- Manage its own WASM memory
- Generate and request reload of new code
- Store/retrieve data via kernel storage API

## Implementation Phases

### Phase 1: Kernel Foundation (Week 1-2)
- [ ] Project workspace structure (kernel/ + runtime/ + codegen/)
- [ ] WASM runtime integration (wasmtime)
- [ ] Error handling and logging
- [ ] Kernel state persistence (SQLite)

### Phase 2: Mutable Runtime Core (Week 2-3)
- [ ] Runtime WASM module structure
- [ ] Host function interface (kernel → runtime)
- [ ] LLM client exposed to WASM
- [ ] Storage API (get/set) for WASM
- [ ] HTTP client exposed to WASM

### Phase 3: Hot Reload & Codegen (Week 3-4)
- [ ] Rust → WASM compilation pipeline
- [ ] Hot reload mechanism (atomic swaps)
- [ ] Version control for runtimes
- [ ] State preservation across reloads
- [ ] Self-coding loop (generate → compile → reload)

### Phase 4: Resource Limits & Safety (Week 4-5)
- [ ] WASM fuel metering (instruction counting)
- [ ] Memory limits enforcement
- [ ] Execution timeouts
- [ ] Code size limits
- [ ] Daily reload quotas

### Phase 5: Tools & Channels (Week 5-6)
- [ ] Tool registry in WASM runtime
- [ ] Built-in tools (shell, file, web, memory)
- [ ] Channel trait in WASM
- [ ] Telegram channel implementation
- [ ] CLI channel for testing

### Phase 6: Self-Healing & Autonomy (Week 6-8)
- [ ] Error detection and capture
- [ ] Self-healing prompt engineering
- [ ] Automatic rollback on failure
- [ ] Performance profiling
- [ ] Self-optimization loops

### Phase 7: Advanced Features (Week 8-10)
- [ ] MCP client integration
- [ ] Subagent delegation
- [ ] Context compression
- [ ] Session search (FTS5)
- [ ] Cron scheduler in runtime

### Phase 8: Polish & Production (Week 10-12)
- [ ] Documentation
- [ ] Example self-generated runtimes
- [ ] Deployment configs
- [ ] Security audit
- [ ] Performance optimization

## Technical Specifications

### Performance Targets
- Kernel binary size: <15MB (including WASM runtime)
- Memory usage: <30MB baseline (Kernel + WASM runtime)
- Startup time: <100ms (WASM runtime loading)
- WASM execution: <10M instructions/sec (with fuel metering)
- Hot reload time: <5s (compile + swap)
- Request latency: <100ms (excluding LLM)

### Kernel Dependencies
```toml
[dependencies]
# Async runtime
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }

# WASM Runtime
wasmtime = { version = "18", features = ["fuel-metering", "async"] }
wasmtime-wasi = "18"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# HTTP client
reqwest = { version = "0.11", features = ["json", "stream"] }

# Database
sqlx = { version = "0.7", features = ["sqlite", "runtime-tokio"] }

# Code generation (for self-coding)
tokio-process = "0.1"  # For calling rustc

# Versioning
semver = "1"

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

### Runtime Dependencies (WASM target)
```toml
# src/runtime/Cargo.toml
[package]
name = "salamander-runtime"
crate-type = ["cdylib"]

[dependencies]
# No std dependencies for WASM
serde = { version = "1", default-features = false, features = ["derive"] }
serde_json = { version = "1", default-features = false }

# Host function bindings (generated)
salamander-host = { path = "../host-bindings" }
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

### File: `~/.config/salamander/config.yaml`
```yaml
# LLM Configuration
provider:
  type: openai
  api_key: "${OPENAI_API_KEY}"
  model: gpt-4o-mini
  temperature: 0.7

# Self-Management
self_management:
  enabled: true
  max_daily_reloads: 100
  auto_rollback: true
  require_confirmation: false

# Resource Limits
resource_limits:
  max_fuel: 10_000_000_000      # WASM instructions
  max_memory: 134217728         # 128MB
  max_execution_time: 30s

# Bot Personality
agent:
  name: "Salamander"
  system_prompt: |
    You are Salamander, a self-managing AI assistant.
    You can write, compile, and reload your own code.
    Always be concise and helpful.

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
data_dir: "~/.local/share/salamander"
```

## Testing Strategy

- **Unit Tests**: Core kernel logic, trait implementations
- **Integration Tests**: End-to-end with mock providers
- **Channel Tests**: Test each channel separately
- **Security Tests**: Fuzzing, penetration testing
- **Self-Coding Tests**: Verify generated code compiles and runs

## References

- **NanoClaw**: Container-first TypeScript implementation (~4K lines)
- **ZeroClaw**: Rust implementation with trait-driven architecture (<5MB RAM)
- **Hermes-Agent**: Python-based autonomous agent with closed learning loop
- **Erlang/Elixir**: Hot code reloading patterns
- **WebAssembly**: wasmtime runtime for sandboxed execution
- **Model Context Protocol**: Anthropic's tool use standard
- **Atropos**: Nous Research RL training framework
