# Manta Architecture

Manta is a personal AI assistant built in Rust. It routes messages from multiple channels through an agent core to LLM providers, with persistent memory, tool execution, and security controls.

## System Overview

```
User Interfaces          Channel Layer           Agent Core              Providers
     |                        |                       |                      |
  CLI / Web           ┌──────────────┐      ┌──────────────┐       ┌──────────────┐
Telegram              │   Channel    │      │   Message    │       │   OpenAI     │
Discord    ────────▶  │   Trait      │ ───▶ │   Router     │  ───▶ │  Anthropic   │
 Slack               │ + Registry   │      │ + Context    │       │  Fallback    │
                      └──────────────┘      └──────────────┘       └──────────────┘
                                                   │
                       ┌───────────────────────────┼───────────────────────────┐
                       ▼                           ▼                           ▼
               ┌──────────────┐          ┌──────────────┐            ┌──────────────┐
               │ Memory Store │          │ Tool System  │            │  Security    │
               │ (Tiered)     │          │ + Registry   │            │  Layer       │
               └──────────────┘          └──────────────┘            └──────────────┘
```

## Modules

| Module | Purpose | Key File |
|--------|---------|----------|
| `agent` | Conversation orchestration, context management, routing | `src/agent/mod.rs` |
| `channels` | Communication interfaces (CLI, Telegram, Discord, Slack, etc.) | `src/channels/mod.rs` |
| `memory` | Persistent storage: conversations, messages, semantic memories, tiered store | `src/memory/mod.rs` |
| `providers` | LLM provider abstractions (OpenAI, Anthropic, streaming) | `src/providers/mod.rs` |
| `tools` | Capabilities the AI can invoke (file, shell, web, MCP, etc.) | `src/tools/mod.rs` |
| `gateway` | Control plane: HTTP/WebSocket API, agent spawning, auth | `src/gateway/mod.rs` |
| `security` | Auth, sandbox, path/command validation | `src/security/mod.rs` |
| `config` | Configuration loading, hot reload, validation | `src/config.rs` |
| `core` | Domain models and shared business logic | `src/core/mod.rs` |

## Data Flow

### Message Processing

```
User ──▶ Channel ──▶ Gateway ──▶ Agent ──▶ LLM Provider
                              │
                              ├──▶ Memory (retrieve context)
                              ├──▶ Tools (if tool calls)
                              └──▶ Security (validate)
```

### Tool Execution

```
LLM ──▶ Tool Call ──▶ Security Validation ──▶ Execute ──▶ Result ──▶ LLM
            │
            └──▶ Approval Queue (human-in-the-loop, if configured)
```

## Key Design Decisions

1. **Trait-based abstraction** — `Channel`, `Provider`, `MemoryStore`, `Tool` are all traits, allowing pluggable implementations.
2. **Arc + dyn for shared state** — `Arc<dyn MemoryStore>` and `Arc<dyn ChatHistoryStore>` enable runtime backend selection (e.g., unified SQLite vs. tiered store).
3. **Feature-gated channels** — Each channel is behind a Cargo feature to keep the binary small.
4. **Tiered memory** — Memories route across Working (in-memory), ShortTerm (SQLite), LongTerm (SQLite), and Archival (compressed JSONL) based on importance.
5. **Context compressor** — Automatically compresses conversation history when approaching token limits.

## Documentation Map

- [`docs/modules/agent.md`](modules/agent.md) — Agent orchestration
- [`docs/modules/channels.md`](modules/channels.md) — Channel interfaces
- [`docs/modules/memory.md`](modules/memory.md) — Memory system
- [`docs/modules/providers.md`](modules/providers.md) — LLM providers
- [`docs/modules/tools.md`](modules/tools.md) — Tool system
- [`docs/modules/gateway.md`](modules/gateway.md) — Gateway control plane
- [`docs/modules/security.md`](modules/security.md) — Security layer
- [`docs/modules/config.md`](modules/config.md) — Configuration
- [`docs/modules/core.md`](modules/core.md) — Domain models
