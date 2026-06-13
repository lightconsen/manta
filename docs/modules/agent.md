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

