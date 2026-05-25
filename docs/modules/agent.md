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
- **📝 Partial**: Full ACP session lifecycle — `ExecutionController::check_and_wait()` is inserted between LLM iterations in the agent tool loop, but pause/resume/step/cancel are exposed as tools rather than lifecycle hooks. See `src/acp/mod.rs:1833-1845` and `src/agent/mod.rs:1971-1992`.
- **📝 Partial**: Subagent delegation — `SubagentRegistry` has spawn/complete/wait/kill with persist/load (`src/agent/subagent_registry.rs`), but `DelegateTool` does not consult the registry for routing decisions.
- **❌ Missing**: Planner results are not persisted across restarts — `TaskPlanner` creates plans in memory with no `plan_store` or save/load methods.
