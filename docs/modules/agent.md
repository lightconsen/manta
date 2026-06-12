# Agent Module

The agent is the central orchestrator that handles conversations, manages context, calls tools, and interacts with LLM providers.

## Design

- **Message Router** (`route_resolution.rs`) — Determines which agent or subagent handles an incoming message.
- **Context Manager** (`context.rs`) — Builds and maintains the conversation context sent to the LLM.
- **Budget System** (`budget.rs`, `disk_budget.rs`, `cost_guard.rs`) — Tracks iteration count, token usage, and disk budget to prevent runaway loops.
- **Compressor** (`compressor.rs`) — Compresses conversation history when approaching token limits.
- **Session Management** (`session.rs`, `session_store.rs`, `session_files.rs`) — Persists and restores agent session state.
- **Todo Store** (`todo.rs`) — Per-session task list that the agent can read and update.
- **Planner** (`planner.rs`) — Decomposes complex user requests into multi-step plans.
- **Prompt Builder** (`prompt_builder.rs`) — Assembles system prompts with personality and tool descriptions.
- **Subagent Registry** (`subagent_registry.rs`) — Tracks spawned subagents for delegation.
- **ACP Integration** (`acp.rs`) — Agent Control Protocol for inter-agent communication.
- **Transcript** (`transcript.rs`) — Records full conversation transcripts for replay.

## Key Types

```rust
pub struct Agent {
    config: AgentConfig,
    memory_store: Option<Arc<dyn MemoryStore>>,
    chat_history: Option<Arc<dyn ChatHistoryStore>>,
    tool_registry: Option<Arc<ToolRegistry>>,
    provider: Option<Arc<dyn Provider>>,
    // ...
}
```

## Data Flow

1. `process_message()` receives an `IncomingMessage`
2. `build_fresh_context()` calls `MemoryManager::session_context()` to retrieve memories + multimodal references + recent messages
3. Context is injected into the LLM prompt
4. LLM response is parsed; if it contains tool calls, they are executed via `ToolRegistry`
5. Tool results are fed back to the LLM
6. Final response is formatted and returned

## Missing / TODO

- **✅ Implemented**: Advanced retry/fallback — `ModelRouter` maintains `fallback_chains` and `get_provider_chain()` for sequential failover across providers. See `src/model_router/mod.rs`.
- **✅ Implemented**: Group chat participant tracking — `GroupSession` with `GroupRole` enum (Owner/Admin/Member/Observer), member management, and `GroupSessionManager`. See `src/agent/group.rs`.
- **✅ Implemented**: Full ACP session lifecycle — `ExecutionController` with `check_and_wait()` is inserted between LLM iterations in the agent tool loop (a true lifecycle hook). pause/resume/step/cancel are exposed through WebSocket RPC (`acp.pause/resume/step/cancel`), ACP bridge commands (`/acp pause` etc.), and ACP actor command handlers. See `src/acp/mod.rs:121-210` (controller), `src/agent/mod.rs:2681-2701` (lifecycle hook), `src/gateway/ws.rs:509-512` (WebSocket handlers), `src/channels/acp_bridge.rs:372+` (channel commands).
- **✅ Implemented**: Subagent delegation with `target_agent` routing — `SubagentRegistry` tracks lifecycle (spawn/complete/wait/kill) with persist/load (`src/agent/subagent_registry.rs`). `DelegateTool` now consults the registry's `AgentResolver` for routing decisions: when `TaskSpec.target_agent` is set, the child task is routed to the named agent via the Gateway's agent pool. Falls back to the default agent when no target is specified or the target is not running. See `src/tools/delegate_tool.rs:182-370` (DelegateTool with resolver), `src/gateway/mod.rs:2182-2208` (registration with Gateway agent pool).
- **✅ Implemented**: Planner persistence — Two systems: (1) `TaskPlanner` uses `PersistedPlan` with JSON file save/load (`src/agent/planner.rs:511-627`); (2) `GoalPlanner` uses `TaskStateStore` with SQLite-backed crash recovery (`src/planner/state.rs`, `src/planner/persistent_queue.rs`). Wired at startup via `load_all_plans()` and `with_planner_state_store()`. See `src/gateway/mod.rs:2578-2612`.
