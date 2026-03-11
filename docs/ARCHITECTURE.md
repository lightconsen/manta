# Manta Architecture

This document describes the high-level architecture of Manta.

## System Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              User Interfaces                                 │
├─────────────┬─────────────┬─────────────┬───────────────────────────────────┤
│    CLI      │  Telegram   │   Discord   │             Slack                 │
│  (rustyline)│  (teloxide) │  (serenity) │          (Web API)                │
└──────┬──────┴──────┬──────┴──────┬──────┴───────────────┬───────────────────┘
       │             │             │                      │
       └─────────────┴─────────────┴──────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                            Channel Layer                                     │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐ │
│  │   Channel    │  │   Message    │  │  Conversation │  │    Formatter     │ │
│  │    Trait     │  │   Handler    │  │      ID       │  │ (Markdown/HTML)  │ │
│  └──────────────┘  └──────────────┘  └──────────────┘  └──────────────────┘ │
└─────────────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                            Agent Core                                        │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                        Agent Orchestration                            │   │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌──────────────┐   │   │
│  │  │   Router   │  │   Context  │  │ Iteration  │  │   Compressor │   │   │
│  │  │            │  │   Manager  │  │   Budget   │  │              │   │   │
│  │  └────────────┘  └────────────┘  └────────────┘  └──────────────┘   │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                        Autonomy Features                              │   │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌──────────────┐   │   │
│  │  │    Todo    │  │    Dual    │  │   Session  │  │   Delegate   │   │   │
│  │  │   Store    │  │   Memory   │  │   Search   │  │     Tool     │   │   │
│  │  └────────────┘  └────────────┘  └────────────┘  └──────────────┘   │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
                              │
              ┌───────────────┼───────────────┐
              ▼               ▼               ▼
┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐
│   Tool System   │ │  LLM Providers  │ │  Memory Store   │
└─────────────────┘ └─────────────────┘ └─────────────────┘
```

## Component Details

### 1. Channel Layer

The channel layer abstracts different communication interfaces:

```
┌──────────────────────────────────────────────────────────┐
│                      Channel Trait                        │
├──────────────────────────────────────────────────────────┤
│  name() -> &str                                          │
│  start(handler: MessageHandler) -> Result<()>            │
│  send(message: OutgoingMessage) -> Result<()>            │
└──────────────────────────────────────────────────────────┘
                              ▲
          ┌──────────────────┼──────────────────┐
          │                  │                  │
    ┌─────┴─────┐     ┌──────┴─────┐    ┌───────┴─────┐
    │   Cli     │     │  Telegram  │    │   Discord   │
    │  Channel  │     │   Channel  │    │   Channel   │
    └───────────┘     └────────────┘    └─────────────┘
```

### 2. Tool System

Tools are capabilities the AI can use:

```
┌─────────────────────────────────────────────────────────────────┐
│                        Tool Registry                             │
├─────────────────────────────────────────────────────────────────┤
│  register(tool: BoxedTool)                                      │
│  get(name: &str) -> Option<&dyn Tool>                           │
│  execute(name: &str, args: Value) -> Result<ToolResult>         │
└─────────────────────────────────────────────────────────────────┘
                              │
        ┌─────────────────────┼─────────────────────┐
        │                     │                     │
   ┌────┴────┐          ┌────┴────┐          ┌────┴────┐
   │  File   │          │  Shell  │          │   Web   │
   │  Tools  │          │  Tool   │          │  Tools  │
   └────┬────┘          └────┬────┘          └────┬────┘
        │                    │                    │
   ┌────┴────┐          ┌────┴────┐          ┌────┴────┐
   │- Read   │          │- Exec   │          │- Fetch  │
   │- Write  │          │- Timeout│          │- Search │
   │- Edit   │          │- Allow  │          │- Validate
   │- Glob   │          │  List   │          │         │
   └─────────┘          └─────────┘          └─────────┘
```

### 3. LLM Provider System

Abstracts different LLM providers:

```
┌─────────────────────────────────────────────────────────────┐
│                     Provider Trait                           │
├─────────────────────────────────────────────────────────────┤
│  complete(request: CompletionRequest) -> CompletionResponse │
│  stream(request: CompletionRequest) -> CompletionStream     │
│  supports_tools() -> bool                                   │
│  max_context() -> usize                                     │
└─────────────────────────────────────────────────────────────┘
                              ▲
         ┌────────────────────┼────────────────────┐
         │                    │                    │
    ┌────┴────┐         ┌────┴────┐        ┌─────┴──────┐
    │ OpenAI  │         │Anthropic│        │  Fallback  │
    │Provider │         │Provider │        │  Provider  │
    └────┬────┘         └────┬────┘        └─────┬──────┘
         │                   │                    │
    ┌────┴────┐         ┌────┴────┐        ┌─────┴──────┐
    │- Chat   │         │- Messages│        │- Chain of  │
    │  API    │         │  API    │        │  providers │
    │- Stream │         │- Stream │        │- Failover │
    │- Tools  │         │- Tools  │        │  logic     │
    └─────────┘         └─────────┘        └────────────┘
```

### 4. Security Layer

Security is implemented at multiple layers:

```
┌─────────────────────────────────────────────────────────────┐
│                     Security Layer                           │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌───────────────────────────────────────────────────────┐ │
│  │                  Input Validation                      │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐   │ │
│  │  │Name Validator│  │Schema Valid │  │Security Valid│   │ │
│  │  └─────────────┘  └─────────────┘  └─────────────┘   │ │
│  └───────────────────────────────────────────────────────┘ │
│                                                              │
│  ┌───────────────────────────────────────────────────────┐ │
│  │                  Security Validators                   │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐   │ │
│  │  │Path Traversal│  │Cmd Injection│  │Rate Limiter │   │ │
│  │  │  Detection   │  │  Detection  │  │             │   │ │
│  │  └─────────────┘  └─────────────┘  └─────────────┘   │ │
│  └───────────────────────────────────────────────────────┘ │
│                                                              │
│  ┌───────────────────────────────────────────────────────┐ │
│  │                  Access Control                        │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐   │ │
│  │  │  Allowlist  │  │   Sandbox   │  │   Auth      │   │ │
│  │  │   Check     │  │   Mode      │  │   Manager   │   │ │
│  │  └─────────────┘  └─────────────┘  └─────────────┘   │ │
│  └───────────────────────────────────────────────────────┘ │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 5. Memory System

Multi-layer memory architecture:

```
┌─────────────────────────────────────────────────────────────┐
│                      Memory Layers                           │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────────────────────────────────────────────────┐│
│  │  Ephemeral Memory (Conversation Context)                 ││
│  │  - Recent messages                                       ││
│  │  - Active tool calls                                     ││
│  │  - Current todo list                                     ││
│  └─────────────────────────────────────────────────────────┘│
│                              │                               │
│                              ▼                               │
│  ┌─────────────────────────────────────────────────────────┐│
│  │  Dual Memory (File-based)                                ││
│  │  ┌───────────────┐      ┌───────────────┐              ││
│  │  │  Procedural   │      │   User Model  │              ││
│  │  │    Memory     │      │               │              ││
│  │  │  (agent.md)   │      │  (user.md)    │              ││
│  │  ├───────────────┤      ├───────────────┤              ││
│  │  │- Tool quirks  │      │- Preferences  │              ││
│  │  │- Conventions  │      │- Communication│              ││
│  │  │- Environment  │      │  style        │              ││
│  │  │  facts        │      │- Habits       │              ││
│  │  └───────────────┘      └───────────────┘              ││
│  └─────────────────────────────────────────────────────────┘│
│                              │                               │
│                              ▼                               │
│  ┌─────────────────────────────────────────────────────────┐│
│  │  Persistent Memory (SQLite)                              ││
│  │  ┌───────────────┐  ┌───────────────┐  ┌─────────────┐ ││
│  │  │ Conversations │  │   Messages    │  │  Memories   │ ││
│  │  ├───────────────┤  ├───────────────┤  ├─────────────┤ ││
│  │  │- ID           │  │- ID           │  │- ID         │ ││
│  │  │- User ID      │  │- Conv ID      │  │- Key        │ ││
│  │  │- Created      │  │- Role         │  │- Value      │ ││
│  │  │- Updated      │  │- Content      │  │- Embedding  │ ││
│  │  └───────────────┘  └───────────────┘  └─────────────┘ ││
│  └─────────────────────────────────────────────────────────┘│
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

## Data Flow

### Message Processing Flow

```
┌─────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│  User   │────▶│  Channel │────▶│  Agent   │────▶│  Router  │
│ Message │     │  Handler │     │  Core    │     │          │
└─────────┘     └──────────┘     └──────────┘     └──────────┘
                                                        │
                       ┌────────────────────────────────┘
                       ▼
┌─────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│  User   │◀────│ Formatter│◀────│  LLM     │◀────│  Tools   │
│ Response│     │          │     │ Provider │     │ (if needed)
└─────────┘     └──────────┘     └──────────┘     └──────────┘
```

### Tool Execution Flow

```
┌──────────┐     ┌──────────┐     ┌──────────────┐
│  LLM     │────▶│  Tool    │────▶│  Security    │
│ Response │     │  Call    │     │  Validation  │
└──────────┘     └──────────┘     └──────────────┘
                                         │
                    ┌────────────────────┘
                    ▼
┌──────────┐     ┌──────────┐     ┌──────────┐
│  LLM     │◀────│ Formatter│◀────│ Execute  │
│ (result) │     │          │     │          │
└──────────┘     └──────────┘     └──────────┘
```

## Module Dependencies

```
main
├── cli
├── config
├── agent
│   ├── context
│   ├── router
│   ├── compressor
│   └── todo
├── channels
│   ├── telegram
│   ├── discord
│   ├── slack
│   └── formatter
├── tools
│   ├── file
│   ├── shell
│   ├── web
│   ├── memory
│   ├── time
│   └── code_exec
├── providers
│   ├── openai
│   ├── anthropic
│   └── fallback
├── memory
│   ├── sqlite
│   ├── vector
│   └── dual
├── security
│   ├── auth
│   └── sandbox
└── utils
    └── logging
```

## Configuration Architecture

```yaml
manta/
├── config.yaml              # Main configuration
├── memory/
│   ├── agent.md            # Procedural memory
│   └── user.md             # User model
├── skills/
│   └── {skill_name}/
│       ├── SKILL.md
│       └── ...
└── data/
    ├── manta.db            # SQLite database
    └── history.txt         # CLI history
```

## Deployment Architecture

### Docker Deployment

```
┌─────────────────────────────────────────────┐
│              Docker Container                │
│  ┌─────────────────────────────────────┐   │
│  │           Manta Service              │   │
│  │  ┌───────┐ ┌───────┐ ┌───────────┐ │   │
│  │  │  CLI  │ │  Bot  │ │  Web API  │ │   │
│  │  └───────┘ └───────┘ └───────────┘ │   │
│  └─────────────────────────────────────┘   │
│  ┌─────────────────────────────────────┐   │
│  │           SQLite Volume              │   │
│  └─────────────────────────────────────┘   │
└─────────────────────────────────────────────┘
```

### Kubernetes Deployment

```
┌─────────────────────────────────────────────────────────────┐
│                        Kubernetes                            │
│  ┌───────────────────────────────────────────────────────┐  │
│  │                      Ingress                           │  │
│  └───────────────────────────────────────────────────────┘  │
│                              │                               │
│  ┌───────────────────────────┼───────────────────────────┐  │
│  │                      Service                           │  │
│  │  (LoadBalancer / ClusterIP)                            │  │
│  └───────────────────────────┼───────────────────────────┘  │
│                              │                               │
│  ┌───────────────────────────┼───────────────────────────┐  │
│  │                   Deployment                           │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐     │  │
│  │  │   Pod 1     │ │   Pod 2     │ │   Pod N     │     │  │
│  │  │ ┌─────────┐ │ │ ┌─────────┐ │ │ ┌─────────┐ │     │  │
│  │  │ │ Manta   │ │ │ │ Manta   │ │ │ │ Manta   │ │     │  │
│  │  │ └─────────┘ │ │ └─────────┘ │ │ └─────────┘ │     │  │
│  │  └─────────────┘ └─────────────┘ └─────────────┘     │  │
│  └───────────────────────────┬───────────────────────────┘  │
│                              │                               │
│  ┌───────────────────────────┼───────────────────────────┐  │
│  │              Persistent Volume Claim                   │  │
│  │                   (SQLite Data)                        │  │
│  └───────────────────────────┴───────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

## Performance Considerations

### Resource Usage Targets

| Component | Target Memory | Notes |
|-----------|---------------|-------|
| Base Binary | <10 MB | Stripped release build |
| Runtime | <20 MB | Without active conversations |
| Per Conversation | ~2-5 MB | Context-dependent |
| SQLite | ~5-10 MB | Connection pooling |

### Optimization Strategies

1. **Context Compression**: Automatic compression when approaching token limits
2. **Lazy Loading**: Tools loaded on-demand
3. **Connection Pooling**: Database connections reused
4. **Caching**: Tool results cached where appropriate
5. **Streaming**: LLM responses streamed to reduce memory

## Future Architecture

Planned enhancements:

1. **Plugin System**: WASM-based plugins for extensibility
2. **Distributed Mode**: Multiple Manta instances with shared state
3. **Vector Database**: Enhanced semantic search capabilities
4. **Model Router**: Intelligent routing between multiple LLM providers
