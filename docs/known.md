# Syscity Codebase Overview

Complete inventory of all Rust source modules, files, and architectural patterns.

---

## Module Summary (12 subsystems, 230 files)

```
src/
├── main.rs / lib.rs          # Entrypoint: CLI/Gateway/Web three-mode launch
├── config.rs                 # Layered config (default → TOML → env SYSCITY_*), hot reload
├── error.rs                  # thiserror unified error taxonomy
├── dirs.rs                   # ~/.syscity/ directory layout
├── secrets.rs                # Multi-source secret resolution (env/file/cmd), zeroize secure memory
├── client.rs                 # HTTP/WS client (CLI → Daemon)
├── web.rs                    # Browser Web Terminal (WS + SSE)
├── daemon.rs                 # Daemon process management (PID file, signals)
│
├── core/                     # Domain models: Entity CRUD engine
│
├── gateway/                  # Control plane: Axum HTTP + WS, JWT auth, OAuth2
│   ├── protocol.rs           # WS RPC protocol (scope-based permission gating)
│   ├── ws.rs                 # WebSocket upgrade + duplex message loop
│   ├── commands.rs           # Slash command catalog
│   ├── auth/                 # Session cookie + GitHub/Google OAuth2
│   ├── middleware.rs         # CORS + CSP + rate limit middleware
│   ├── rate_limit.rs         # Multi-tier sliding window + token bucket
│   ├── send_policy.rs        # Send policy engine (allow/deny/silence)
│   ├── hooks.rs              # Event interception pipeline (chain of responsibility)
│   └── webhooks.rs           # WhatsApp/Telegram/Slack/Feishu webhooks
│
├── inbound/                  # Inbound pipeline
│   ├── debounce.rs           # Per-conversation-key debounce
│   ├── media.rs              # Attachment classification (optional vision model routing)
│   ├── dispatch.rs           # Auto-reply dispatch (policy → plugin → group suppression)
│   ├── queue.rs              # Queue modes: Interrupt/Steer/FollowUp/Collect
│   └── router.rs             # Multi-agent routing (@mention → binding → default)
│
├── outbound/                 # Outbound pipeline
│   ├── trajectory.rs         # Execution trace logging
│   ├── sse.rs                # Per-session SSE stream
│   ├── reply_dispatcher.rs   # Response routing back to channel
│   └── side_effects.rs       # Async side effects (memory, cron, webhook)
│
├── channels/                 # Channel system (feature-gated)
│   ├── mod.rs                # Channel trait: start/stop/send/edit/delete/health
│   ├── telegram.rs           # teloxide bot
│   ├── discord.rs            # serenity gateway
│   ├── slack.rs              # Socket Mode
│   ├── whatsapp.rs           # Meta Business API
│   ├── qq.rs / lark.rs       # Tencent / Feishu
│   ├── signal.rs             # signal-cli JSON-RPC
│   ├── imessage.rs           # BlueBubbles REST+WS
│   ├── webchat.rs            # Built-in HTTP WS server
│   ├── extension.rs          # ChannelExtension adapter pattern
│   ├── telegram_extension.rs # Telegram extension bridge
│   ├── plugin_host.rs        # WASM third-party channel runtime (wasmtime)
│   ├── formatter.rs          # Markdown → HTML/Discord/Slack conversion
│   ├── health.rs             # Health checks + staleness detection
│   ├── lifecycle.rs          # Exponential backoff auto-restart
│   ├── metrics.rs            # Rolling window percentile metrics
│   └── state.rs              # SQLite channel state persistence
│
├── agent/                    # Agent core
│   ├── mod.rs                # Agent facade
│   ├── context.rs            # Message history + token counting + pruning
│   ├── turns.rs              # Turn branching + undo/redo (command pattern)
│   ├── budget.rs             # Iteration budget (atomic counter)
│   ├── disk_budget.rs        # Session storage quota + LRU eviction
│   ├── cost_guard.rs         # Live cost tracking (atomic flags)
│   ├── session.rs            # Multi-agent session orchestration (Actor model mpsc)
│   ├── session_store.rs      # SQLite session persistence
│   ├── session_files.rs      # Session-isolated filesystem (path traversal protection)
│   ├── subagent_registry.rs  # Subagent lifecycle (depth/concurrency limits)
│   ├── route_resolution.rs   # Multi-dimensional message routing (rule engine + TTL cache)
│   ├── acp.rs                # ACP anti-corruption layer (re-exports crate::acp)
│   ├── prompt_builder.rs     # Dynamic system prompt (priority-based pruning)
│   ├── personality.rs        # SOUL.md/IDENTITY.md personality discovery
│   ├── planner.rs            # LLM task decomposition (heuristic vs LLM)
│   ├── todo.rs               # Task state machine (Pending→InProgress→Completed)
│   ├── compaction.rs         # Pre-compaction flush (SHA-256 dedup)
│   ├── compressor.rs         # Context compression (strategy pattern: oldest/summarize/sliding)
│   ├── transcript.rs         # Multi-format conversation export
│   ├── artifacts.rs          # Session artifact tracking
│   └── group.rs              # Multi-user group sessions (RBAC)
│
├── memory/                   # Memory system
│   ├── mod.rs                # MemoryStore + ChatHistoryStore trait
│   ├── tier.rs               # Four tiers: Working/ShortTerm/LongTerm/Archival
│   ├── tiered_store.rs       # Aggregate router (delegates by tier)
│   ├── in_memory_store.rs    # Ephemeral HashMap
│   ├── db.rs                 # SQLite (WAL + FTS5 + embedding BLOB)
│   ├── compressed_store.rs   # gzip JSONL daily shards
│   ├── manager.rs            # Facade: stores + embeddings + hybrid search + context
│   ├── hybrid.rs             # Semantic + BM25 + MMR rerank + temporal decay
│   ├── vector.rs             # Vector backend + embedding providers + text chunking
│   ├── local_embeddings.rs   # Local GGUF (llama.cpp + HF download)
│   ├── pipeline.rs           # Async embedding pipeline (mpsc + dual-trigger batching)
│   ├── events.rs             # JSONL event log (Recall/Promotion/Compact/Dream)
│   ├── dreaming.rs           # Sleep phases: Light dedup → Deep clustering → REM knowledge graph
│   ├── effectiveness.rs      # Recall hit-rate tracking (not yet wired)
│   ├── session_search.rs     # Session history FTS5 search
│   ├── qmd.rs                # Query Markdown scope control
│   ├── multimodal.rs         # File classification (image/audio)
│   ├── personality.rs        # Personality memory
│   ├── soul.rs               # SOUL file management
│   ├── workspace_state.rs    # Workspace state
│   └── flush.rs              # Memory flush
│
├── providers/                # LLM providers
│   ├── mod.rs                # Provider trait + message/tool/stream types
│   ├── openai.rs             # SSE stream + exponential backoff retry
│   ├── anthropic.rs          # Messages API + SSE
│   ├── fallback.rs           # Sequential failover decorator
│   ├── stream_wrappers.rs    # Stream family registry (reasoning extract/tool accumulate/JSON repair)
│   └── sdk.rs                # Provider SDK extension
│
├── model_router/             # Model routing
│   ├── mod.rs                # Alias upgrade + circuit breaker + cost-aware routing
│   ├── auth_profile.rs       # Multi-key rotation (cooldown/permanent disable)
│   ├── auth_profile_store.rs # SQLite key metadata
│   ├── failure_class.rs      # HTTP status → error classification
│   ├── gateway_client.rs     # Unified HTTP client (TLS fingerprint)
│   ├── model_catalog.rs      # Dynamic model catalog (static + plugin discovery)
│   ├── oauth_*.rs            # OAuth2 + PKCE full flow
│   ├── pkce.rs               # S256 challenge
│   ├── usage_fetcher.rs      # Remote quota fetching
│   ├── usage_formatter.rs    # Human-readable usage report
│   └── usage_tracker.rs      # Time-window usage tracking + budget gate
│
├── tools/                    # Tool system (34 files)
│   ├── mod.rs                # Tool trait + registry + execution flow + circuit breaker
│   ├── approval.rs           # Human-in-the-loop approval queue (oneshot resume)
│   ├── hooks.rs              # Policy hooks (Allow/Deny/NeedsApproval)
│   ├── sandbox.rs            # Decorator: path/network restriction + timeout
│   ├── sdk.rs                # Dynamic tool pack registration
│   ├── file.rs               # Read/write/edit/glob (path safety)
│   ├── shell.rs              # Sandboxed shell (RLIMIT)
│   ├── grep.rs               # Regex search
│   ├── patch.rs              # git apply patch
│   ├── process.rs            # Background process management
│   ├── pdf.rs                # Markdown → HTML → PDF
│   ├── image.rs              # Exif + DALL-E 3
│   ├── web.rs                # HTTP fetch + web search (DDG/Bing/Google/Brave)
│   ├── browser.rs            # 25+ Chrome actions (chromiumoxide, feature-gated)
│   ├── nodes.rs              # Tailscale device discovery/remote control
│   ├── memory.rs             # Memory CRUD + semantic search
│   ├── todo_tool.rs          # Per-conversation task list
│   ├── update_plan.rs        # Ordered execution plan
│   ├── time.rs               # Natural language time parsing
│   ├── cron_tool.rs          # Cron scheduling (global OnceCell)
│   ├── delegate_tool.rs      # Child agent spawn (depth≤2, max 3 children)
│   ├── acp_tool.rs           # ACP subagent/session management
│   ├── agents_list.rs        # Available agent list
│   ├── team_communicate_tool.rs # Team messaging (Mesh/Star/Chain/Broadcast)
│   ├── session.rs            # Session list/history/send/yield
│   ├── message.rs            # Channel message (send/edit/delete/react/poll)
│   ├── gateway.rs            # Gateway restart/config inspect/mutation
│   ├── mcp.rs                # MCP client (stdio/SSE/streamable-http)
│   ├── code_exec.rs          # Sandboxed Python (forbidden import check)
│   ├── tts.rs                # TTS (OpenAI → say → espeak fallback)
│   └── canvas.rs             # A2UI canvas control
│
├── security/                 # Security
│   ├── mod.rs                # AuthManager + rate limit + secret scanning
│   ├── audit.rs              # Comprehensive security audit (risk scoring)
│   ├── runtime_audit.rs      # In-memory ring buffer
│   ├── persistent_audit.rs   # SQLite audit log
│   ├── pairing.rs            # DM pairing (OpenClaw-style)
│   ├── device_pairing.rs     # WS device pairing (token auth)
│   ├── mention_gate.rs       # Mention gating policy
│   ├── sliding_window.rs     # Sliding window rate limiter
│   └── pentest.rs            # Dynamic penetration testing (pluggable probes)
│
├── skills/                   # Skill system
│   ├── mod.rs                # Skill manager (OpenClaw-compatible SKILL.md)
│   ├── builtin.rs            # 13 built-in skills
│   ├── builtin_macros.rs     # Compile-time skill embedding macros
│   ├── registry.rs           # Remote skill registry client
│   ├── config.rs             # User skill config
│   ├── install.rs            # Multi-package-manager install (Brew/Npm/Go/Uv/Cargo/Shell)
│   ├── storage.rs            # Tiered storage (Bundled/User/Workspace/Project)
│   ├── watcher.rs            # Filesystem hot reload
│   ├── frontmatter.rs        # SKILL.md YAML frontmatter parser
│   ├── dependencies.rs       # Dependency graph + topological sort
│   └── semver.rs             # Semantic version parsing
│
├── plugins/                  # Plugin system
│   ├── mod.rs                # Plugin manager
│   ├── manifest.rs           # plugin.json capability model
│   ├── runtime.rs            # WASM sandboxed execution (wasmtime)
│   ├── hooks.rs              # Plugin hook registry
│   └── provider_extension.rs # Plugin provider adapter
│
├── taskflow/                 # Durable execution engine
│   ├── engine.rs             # Checkpoint/resume/retry
│   ├── state.rs              # State machine
│   └── store.rs              # SQLite checkpoint persistence
│
├── team/                     # Team
│   ├── mod.rs                # Team management (hierarchy + communication patterns)
│   ├── assistant_mesh.rs     # Generic agent message router
│   └── mesh.rs               # Team + Mesh integration runtime
│
├── browser/                  # Browser automation (feature-gated)
│   ├── sandbox.rs            # Docker isolation + noVNC
│   ├── aria_snapshot.rs      # Accessible tree extraction (ref-marked interaction)
│   ├── bridge.rs             # Axum REST API bridge
│   ├── bridge_client.rs      # HTTP client
│   ├── navigation_guard.rs   # SSRF prevention
│   ├── pool.rs               # Persistent instance pool + idle eviction
│   └── profile.rs            # Chrome profile (headless/headed/MCP)
│
├── canvas/                   # A2UI dynamic UI
│   └── mod.rs                # Components/events/updates broadcast + WS push
│
├── cron/                     # Cron scheduling
│   └── cron.rs               # Actor command channel + exponential backoff + JSONL run log
│
├── server/                   # HTTP server
│   └── mod.rs                # Axum REST + WS endpoints
│
├── tailscale/                # Tailscale integration
│   └── mod.rs                # serve/funnel CLI wrapper
│
├── adapters/                 # External adapters
│   ├── api.rs                # HTTP client (retry + secret resolution)
│   └── storage.rs            # Storage trait (memory/file/SQLite)
│
├── export/                   # Export
│   ├── formats.rs            # Markdown/JSON/JSONL formats
│   └── service.rs            # Export orchestration
│
├── utils/                    # Utilities
│   ├── logging.rs            # tracing subscriber init
│   ├── pool.rs               # Connection pool management
│   ├── batch.rs              # Batch processing + dedup (Actor + oneshot)
│   └── profiling.rs          # Performance timer + memory stats
│
└── cli/                      # CLI (23 subcommands)
    ├── mod.rs                # clap derive top-level command
    ├── chat.rs               # Single-shot / interactive chat
    ├── agent.rs              # Agent personality management
    ├── session.rs            # Session/thread/turn undo-redo
    ├── skill.rs              # Skill lifecycle
    ├── admin.rs              # Gateway management
    ├── approval.rs           # Approval queue
    ├── audit.rs              # Audit logs
    ├── channel.rs            # Channel configuration
    ├── config_cmd.rs         # Config get/set/show
    ├── cron.rs               # Cron management
    ├── daemon.rs             # Daemon lifecycle
    ├── device.rs             # Device pairing
    ├── doctor.rs             # Diagnostic system (pluggable)
    ├── entity.rs             # Entity CRUD
    ├── export.rs             # Local export
    ├── mcp.rs                # MCP server management
    ├── memory.rs             # Vector search
    ├── plugin.rs             # WASM plugin management
    ├── provider.rs           # Provider management + OAuth
    ├── security.rs           # Security audit + pairing
    ├── setup.rs              # Interactive setup wizard
    └── team.rs               # Team management
```

---

## Key Architectural Patterns

| Pattern | Applied To |
|---|---|
| **Trait polymorphism** | `Channel`, `Provider`, `MemoryStore`, `Tool`, `Storage`, `TaskExecutor`, `Probe` |
| **Actor model** | Session Actor (mpsc), Cron Scheduler (mpsc + Notify), Batch Processor (mpsc + oneshot) |
| **Registry pattern** | `ChannelRegistry`, `ToolRegistry`, `PluginManager`, `HookRegistry`, `ModelCatalog` |
| **Decorator** | `FallbackProvider`, `SandboxedTool`, `CachedEmbeddingProvider` |
| **Facade** | `MemoryManager`, `ExportService`, `GatewayState` |
| **State machine** | `TurnState`, `TaskStatus`, `AgentInstanceStatus`, `JobState`, `DreamPhase` |
| **Strategy** | Compression strategy, eviction strategy, communication patterns (Mesh/Star/Chain/Broadcast) |
| **Builder** | `HotReloadBuilder`, `AgentBuilder`, `MemoryManagerBuilder`, `PromptBuilder` |
| **Sandbox** | WASM plugins (wasmtime), Docker browser, Shell RLIMIT, Python forbidden-import check |
| **Pipeline** | Inbound (debounce → media → dispatch → queue → router), Outbound (trajectory → SSE → reply → side effects) |
