# Manta Agent vs OpenClaw Agent — Detailed Comparison

> Last updated: 2026-05-15

## Overview

Both Manta and OpenClaw provide a multi-agent runtime with LLM orchestration, tool calling, context management, and session handling. OpenClaw's agent system is built around the ACP (Agent Control Plane) with actor queues in TypeScript, while Manta's is implemented in Rust modules under `src/agent/` with Tokio async primitives.

**Current alignment: ~90%**

---

## Core Agent Architecture

| Feature | OpenClaw | Manta | Status |
|---------|----------|-------|--------|
| **Runtime** | ACP (Agent Control Plane) + actor queue | Tokio async + mpsc channels | Aligned concept |
| **Agent Modes** | `run` (one-shot) / `session` (persistent) | Single persistent mode + subagent spawning | Manta simplified |
| **Agent Struct** | `Agent` with `ACPSessionManager` | `Agent` with `thread_map`, `tools`, `provider` | Aligned |
| **Agent Config** | Model overrides, level overrides | `AgentConfig` with system_prompt, temperature, max_tokens | Aligned |
| **Agent Handle** | `AgentHandle` with command channel | `Agent` cloned per use (Arc-based) | Manta differs |
| **Progress Events** | EventEmitter-based | `ProgressEvent` enum with callback | Aligned |
| **Shutdown** | `Shutdown` command in actor queue | `shutdown_tx: mpsc::Sender<()>` | Aligned |

### Key Differences

- **ACP vs Direct Async**: OpenClaw uses a centralized ACP with `SessionActorQueue` for all agent orchestration. Manta uses direct Tokio async with `Arc<Agent>` shared across tasks, which is simpler but lacks ACP-level centralized control.
- **Modes**: OpenClaw explicitly supports `run` (one-shot, no persistence) and `session` (persistent with thread binding). Manta currently only supports persistent mode; one-shot mode would be a thin wrapper that discards context after completion.

---

## Personality System

| Feature | OpenClaw | Manta (`personality.rs`) | Status |
|---------|----------|--------------------------|--------|
| **Personality Files** | SOUL.md, IDENTITY.md, BOOTSTRAP.md | Same + AGENTS.md, TOOLS.md, HEARTBEAT.md, MEMORY.md, USER.md | Manta enhanced |
| **File Loading** | Dynamic discovery from `agents/` dir | `AgentRegistry::discover()` from `agents/` dir | Aligned |
| **Prompt Building** | Static concatenation | `build_system_prompt()` with priority ordering | Aligned |
| **Subagent Prompts** | Basic | `PersonalityContext::Subagent` excludes Bootstrap/Heartbeat/Memory | Manta enhanced |
| **Task Matching** | Keyword-based in ACP | `can_handle()` with keyword scoring | Aligned |
| **Display Name** | From IDENTITY.md first line | `display_name()` from `# Title` or `Name:` | Aligned |
| **Max File Size** | Not enforced | 4KB truncation per file | Manta safer |

### Key Differences

- **Extra Files**: Manta adds `HEARTBEAT.md` (periodic tasks), `MEMORY.md` (curated long-term memory), and `USER.md` (user preferences) for richer personality context.
- **Context-Aware Prompts**: Manta's `to_agent_config_for(PersonalityContext::Subagent)` intentionally omits startup-only and personal-context sections to reduce token waste for spawned subagents and cron jobs.

---

## Context Management

| Feature | OpenClaw | Manta (`context.rs`) | Status |
|---------|----------|----------------------|--------|
| **Message History** | Flat array in ACP session | `Vec<Message>` in `Context` | Aligned |
| **Token Counting** | Approximate (4 chars/token) | Same approximation | Aligned |
| **Pruning** | Basic truncation | `prune_if_needed()` with tool-call pair protection | Manta enhanced |
| **Turn Limit** | Not explicit | `max_turns` hard cap on user+assistant pairs | Manta extra |
| **Tool Call Deduplication** | Not implemented | `executed_tool_calls: HashSet` prevents loops | Manta extra |
| **Tool Iteration Limit** | Not explicit | `tool_iterations` counter with dynamic limit | Manta extra |
| **Summarization** | Integrated in ACP | `summarize()` heuristic + `ContextCompressor::compact_with_llm()` | Aligned |
| **Stale Detection** | Not explicit | `is_stale()` with max_age | Manta extra |

### Key Differences

- **Tool Call Protection**: Manta's `prune_if_needed()` explicitly protects pending tool call pairs (assistant message with `tool_use` + tool result) to avoid API errors from orphaned tool results.
- **Dynamic Tool Limit**: Manta calculates a dynamic tool iteration limit based on message complexity (length, comma-separated parts, "steps" keywords), capped at 30, overridable via `MANTA_MAX_TOOL_ITERATIONS` env var.

---

## Thread / Turn Model

| Feature | OpenClaw | Manta (`turns.rs`) | Status |
|---------|----------|--------------------|--------|
| **Thread Concept** | `threadId` in binding | `Thread` struct with id, label, context | Manta richer |
| **Turn Log** | Implicit in message history | `Vec<Turn>` with lifecycle states | Manta extra |
| **Undo/Redo** | Not implemented | `undo_last_turn()` / `redo_last_turn()` | Manta extra |
| **Thread Manager** | Not explicit | `ThreadManager` for multi-thread sessions | Manta extra |
| **Turn States** | N/A | `Pending`, `Running`, `Complete`, `Interrupted`, `Error` | Manta extra |
| **Thread Binding** | `PersistentBinding` { threadId, agentId, mode } | `ThreadBinding` enum (Isolated/Parent/Existing/Shared) | Aligned |

### Key Differences

- **Undo/Redo**: Manta's `Thread` maintains a `redo_stack` so undone turns can be restored until new input invalidates the history. OpenClaw has no equivalent.
- **Turn Lifecycle**: Manta tracks explicit turn states (Pending → Running → Complete/Interrupted/Error) which enables pause/resume and error recovery patterns not present in OpenClaw.

---

## Task Planning

| Feature | OpenClaw | Manta (`planner.rs`) | Status |
|---------|----------|----------------------|--------|
| **Planner** | Integrated into ACP | `TaskPlanner` with LLM decomposition | Aligned |
| **Plan Structure** | Implicit | `TaskPlan` { id, goal, tasks[], current_task_index } | Manta explicit |
| **Task Dependencies** | Supported | `PlannedTask.dependencies: Vec<String>` | Aligned |
| **Complexity Detection** | ACP-level heuristics | `needs_planning()` with keyword + LLM check | Manta enhanced |
| **Plan Progress** | Not explicit | `progress_percent()`, `format_summary()` | Manta extra |
| **Active Plans** | Not explicit | `HashMap<String, ActivePlan>` per conversation | Manta extra |

### Key Differences

- **Complexity Detection**: Manta uses a two-stage approach: quick keyword heuristic ("steps", "plan", "implement", etc.) for fast-path rejection, then LLM classification for edge cases.
- **Plan Visualization**: Manta's `format_summary()` produces a human-readable plan with emoji status indicators (✅/🔄/⏳) that can be shown to users.

---

## Prompt Builder

| Feature | OpenClaw | Manta (`prompt_builder.rs`) | Status |
|---------|----------|-----------------------------|--------|
| **Dynamic Prompts** | Model overrides, level overrides | `PromptBuilder` with `PromptSection` priorities | Aligned |
| **Task Type Detection** | Implicit in ACP | `detect_task_type()` → `TaskType` enum | Manta explicit |
| **Conversation Phase** | Not explicit | `ConversationPhase` (New/Early/Established/Deep) | Manta extra |
| **Tool Context** | Static | Dynamic `tool_defs` with `ToolContext` + skill trust | Manta enhanced |
| **Memory Injection** | Integrated | `memory_context` retrieved via `MemoryManager` | Aligned |
| **Skills Filtering** | Not implemented | `SkillManager::prefilter_skills()` by trigger match | Manta extra |
| **Task Context** | Not explicit | `task_context` from active plan injected into prompt | Manta extra |

### Key Differences

- **Task-Specific Instructions**: Manta's `TaskType` enum (Coding, Debugging, Explanation, Writing, Research, System, Planning, FollowUp) provides task-specific system prompt appendices that OpenClaw lacks.
- **Skill Trust Levels**: Manta's `active_skill_trust` atomic (Community=0 / Trusted=1) gates which tools are available, enabling safe skill sandboxing.

---

## Cost Guard / Budget

| Feature | OpenClaw | Manta (`cost_guard.rs`, `budget.rs`) | Status |
|---------|----------|--------------------------------------|--------|
| **Daily Spend Limit** | Basic | `CostGuard` with daily_cents + hourly_actions | Manta enhanced |
| **Model Pricing** | Not explicit | Hardcoded pricing table (Opus/Sonnet/Haiku/GPT-4o) | Manta extra |
| **Auto Reset** | Not explicit | Daily (24h) + hourly (1h) auto-reset | Manta extra |
| **Atomic Checks** | Not explicit | `budget_exceeded: AtomicBool` checked before every provider call | Manta extra |
| **Iteration Budget** | Not explicit | `IterationBudget` with shared counter (parent/child) | Manta extra |
| **Budget Actions** | Not explicit | `BudgetExhaustionAction` (Error/ReturnPartial/AskUser) | Manta extra |

### Key Differences

- **Live Cost Tracking**: Manta's `CostGuard` estimates cost per provider call using model-specific pricing and trips an atomic flag when limits are exceeded. This is checked before every `get_completion()` call.
- **Iteration Budget**: Manta's `IterationBudget` uses an `Arc<AtomicUsize>` so parent and child agents share the same counter, preventing runaway subagent chains from consuming unbounded iterations.

---

## Response Caching

| Feature | OpenClaw | Manta (`mod.rs`) | Status |
|---------|----------|------------------|--------|
| **Cache Strategy** | Not implemented | LLM-driven cacheability classification | Manta extra |
| **Cache Key** | N/A | Hash of user_id + conversation_id + message | Manta extra |
| **TTL** | N/A | Configurable (default 1 hour) | Manta extra |
| **Time-Sensitive Detection** | Not explicit | `is_obviously_time_sensitive()` fast-path + LLM check | Manta extra |
| **Tool-Based Invalidation** | N/A | `are_tools_cacheable()` skips cache for time-sensitive tools | Manta extra |
| **Cache Cleanup** | N/A | Automatic cleanup when >1000 entries | Manta extra |

### Key Differences

- **LLM Cache Classification**: Manta asks the LLM to classify whether a query is cacheable ("CACHE" vs "NOCACHE") rather than using rigid rules. This handles edge cases like "explain quantum computing" (cacheable) vs "what time is it" (not cacheable).
- **Tool-Aware Caching**: Responses that used tools like `datetime`, `weather_current`, or `stock_price` are automatically excluded from caching regardless of LLM classification.

---

## Context Compression / Compaction

| Feature | OpenClaw | Manta (`compressor.rs`, `compaction.rs`) | Status |
|---------|----------|------------------------------------------|--------|
| **Compression Strategies** | Integrated in ACP | `OldestFirst`, `Summarize`, `SlidingWindow` | Aligned |
| **Message Priorities** | Not explicit | `MessagePriority` (Critical/High/Normal/Low) | Manta extra |
| **LLM Compression** | Integrated | `compact_with_llm()` asks LLM to summarize mid-section | Aligned |
| **Memory Flush** | Similar concept | `MemoryFlushConfig` with soft threshold + force threshold | Aligned |
| **Flush Deduplication** | Not explicit | SHA-256 context hash + compaction count tracking | Manta enhanced |
| **Pre-Compaction Turn** | Similar | Silent agent turn to extract durable memories | Aligned |

### Key Differences

- **Priority-Aware Compression**: Manta's `ContextCompressor` assigns `MessagePriority` based on role and recency, ensuring system prompts and recent tool results are never pruned.
- **Flush State Tracking**: Manta's `SessionCompactionState` tracks compaction count and context hash to prevent redundant memory flushes within the same compaction cycle.

---

## Subagent System

| Feature | OpenClaw | Manta (`subagent_registry.rs`) | Status |
|---------|----------|--------------------------------|--------|
| **Spawning** | `spawnSubagent(mode)` | `SubagentRegistry::spawn()` with task_fn closure | Aligned |
| **Thread Binding** | Persistent binding with threadId | `child_session` auto-generated from parent | Aligned |
| **Depth Limit** | Supported | `max_depth` enforced at spawn time | Aligned |
| **Concurrency Limit** | Supported | `max_concurrent` enforced at spawn time | Aligned |
| **Wait/Completion** | Promise-based | `wait_for_completion()` with timeout | Aligned |
| **Kill** | Supported | `kill()` sets `Killed` status | Aligned |
| **Metrics** | Basic | `SubagentMetrics` { spawned, completed, failed, killed } | Manta explicit |
| **Persistence** | Not explicit | `persist_to()` / `load_from()` NDJSON for crash recovery | Manta extra |
| **Run Records** | Not explicit | `RunRecord` with `SystemTime` for serialization | Manta extra |

### Key Differences

- **Closure-Based Execution**: Manta's `spawn()` takes a `task_fn` closure that receives `run_id` and `task`, allowing flexible execution patterns (direct agent call, background job, etc.).
- **Crash Recovery**: Manta's `SubagentRegistry` can persist completed/failed runs to NDJSON and reload them on restart, which OpenClaw lacks.

---

## Todo / Task Tracking

| Feature | OpenClaw | Manta (`todo.rs`) | Status |
|---------|----------|-------------------|--------|
| **Task Model** | Not implemented | `Task` with status, priority, subtasks | Manta extra |
| **Task Status** | N/A | `Pending`, `InProgress`, `Completed`, `Cancelled` | Manta extra |
| **Subtasks** | N/A | `parent_id` + `subtasks: Vec<String>` | Manta extra |
| **Priority** | N/A | 1-5 scale with clamping | Manta extra |
| **Metadata** | N/A | `HashMap<String, serde_json::Value>` | Manta extra |
| **TodoStore** | N/A | In-memory store with CRUD | Manta extra |

### Key Differences

- Manta has a full todo/task tracking system that OpenClaw lacks. Tasks can have subtasks, priorities, and metadata. The `TodoStore` is available to agents via the `Agent` struct.

---

## Multi-Agent Session

| Feature | OpenClaw (`group.ts`, `session.ts`) | Manta (`session.rs`) | Status |
|---------|-------------------------------------|----------------------|--------|
| **Session Model** | `Session` with `agents[]` | `MultiAgentSession` with `agents: HashMap` | Aligned |
| **Agent Lifecycle** | Spawn/terminate via messages | `SessionMessage::{SpawnAgent, TerminateAgent}` | Aligned |
| **Thread Binding** | `ThreadMode` (isolated, parent, shared) | `ThreadBinding` (Isolated, Parent, Shared, Existing) | Manta +1 variant |
| **Intent Routing** | Keyword-based routing | `find_agent_for_intent()` | Aligned |
| **Message Routing** | Route / Broadcast | `RouteToAgent` / `Broadcast` | Aligned |
| **Status Query** | Not explicit | `GetStatus` with oneshot reply | Manta extra |
| **Session Manager** | `SessionManager` | `SessionManager` with timeout cleanup | Aligned |
| **Background Task** | Actor queue | `tokio::spawn(session_processing_task)` | Aligned |

**Note**: See `docs/diffs/session.md` for the full session mechanism comparison (transcripts, artifacts, disk budget, group sessions, route resolution).

---

## Manta-Exclusive Agent Features (Not in OpenClaw)

| Feature | Module | Description |
|---------|--------|-------------|
| **Thread/Turn Undo-Redo** | `turns.rs` | `undo_last_turn()` / `redo_last_turn()` with redo stack |
| **Dynamic Tool Iteration Limit** | `context.rs` | Complexity-based limit (10-30) with env override |
| **Tool Call Deduplication** | `context.rs` | `HashSet` prevents identical tool calls in one turn |
| **LLM Cache Classification** | `mod.rs` | Asks LLM whether query is cacheable |
| **Skill Trust Levels** | `mod.rs` | Atomic `active_skill_trust` gates tool availability |
| **Skill Prefiltering** | `mod.rs` | `SkillManager` filters skills by trigger match before prompt |
| **Cost Guard** | `cost_guard.rs` | Daily/hourly spend + action rate limits with atomic flags |
| **Iteration Budget** | `budget.rs` | Shared atomic counter for parent/child agents |
| **Todo System** | `todo.rs` | Task tracking with subtasks, priority, metadata |
| **PersonalityContext** | `personality.rs` | Primary vs Subagent prompt variants |
| **HEARTBEAT.md / MEMORY.md** | `personality.rs` | Extra personality files for periodic tasks and curated memory |

---

## OpenClaw-Exclusive Agent Features (Not in Manta)

| Feature | Module | Gap |
|---------|--------|-----|
| **ACP Actor Queue** | `acp/` | Centralized orchestration with `SessionActorQueue` |
| **Run Mode** | `acp-spawn.ts` | One-shot execution mode (`run` vs `session`) |
| **Runtime Controls** | `acp/` | Pause/resume/step-through agent execution |
| **Voice/TTS Integration** | `tts/` | Large gap — requires TTS engine |
| **Plugin Tools** | `plugins/` | Dynamic tool registration via jiti |

---

## File Mapping

| OpenClaw File | Manta File | Lines |
|---------------|------------|-------|
| `agents/` (550+ files) | `src/agent/mod.rs` | ~1,400 |
| `acp/acp-spawn.ts` | `src/agent/subagent_registry.rs` | ~597 |
| `acp/session-actor.ts` | `src/agent/session.rs` | ~584 |
| `personality/` | `src/agent/personality.rs` | ~525 |
| `planner.ts` | `src/agent/planner.rs` | ~280 |
| `prompt-builder.ts` | `src/agent/prompt_builder.rs` | ~320 |
| `context.ts` | `src/agent/context.rs` | ~527 |
| `compaction.ts` | `src/agent/compaction.rs` | ~200 |
| `compressor.ts` | `src/agent/compressor.rs` | ~280 |
| `budget.ts` | `src/agent/budget.rs` | ~161 |
| `cost-guard.ts` | `src/agent/cost_guard.rs` | ~209 |
| `todo.ts` | `src/agent/todo.rs` | ~250 |
| `turns.ts` | `src/agent/turns.rs` | ~696 |
| N/A | `src/agent/session_store.rs` | ~960 |
| N/A | `src/agent/group.rs` | ~508 |
| N/A | `src/agent/route_resolution.rs` | ~645 |
| N/A | `src/agent/transcript.rs` | ~648 |
| N/A | `src/agent/artifacts.rs` | ~487 |
| N/A | `src/agent/disk_budget.rs` | ~405 |

**Total**: OpenClaw ~20,000 lines (TypeScript) vs Manta ~9,000 lines (Rust) across agent-related files.

---

## Summary

Manta's agent system is **functionally equivalent** to OpenClaw's with several enhancements:

1. **Richer Personality**: 8 markdown files vs OpenClaw's 3, with context-aware prompt variants
2. **Thread/Turn Model**: Undo/redo support with explicit turn lifecycle states
3. **Cost Controls**: Live spend tracking with model-specific pricing
4. **Response Cache**: LLM-driven cacheability with tool-aware invalidation
5. **Tool Safety**: Deduplication + dynamic iteration limits + skill trust levels
6. **Task Planning**: Explicit `TaskPlan` with progress tracking and dependency management
7. **Todo System**: Full task tracking with subtasks and priorities
8. **Context Compression**: Priority-aware pruning with LLM-assisted compaction

The remaining ~10% gap is primarily in:
- **ACP centralized orchestration** (actor queue pattern)
- **Run mode** (one-shot execution)
- **Runtime controls** (pause/resume/step)
- **Voice/TTS integration**
- **Plugin tool system**
