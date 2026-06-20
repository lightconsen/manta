# Agent Module

The central orchestrator that handles conversations, manages context, calls tools, and interacts with LLM providers.

## Design

- **`route_resolution.rs`** — Determines which agent or subagent handles an incoming message.
- **`context.rs`** — Builds and maintains the conversation context sent to the LLM.
- **`budget.rs`** / **`disk_budget.rs`** / **`cost_guard.rs`** — Tracks iteration count, token usage, and disk budget to prevent runaway loops.
- **`compressor.rs`** — Compresses conversation history when approaching token limits.
- **`session.rs`** / **`session_store.rs`** / **`session_files.rs`** — Persists and restores agent session state.
- **`todo.rs`** — Per-session task list that the agent can read and update.
- **`planner.rs`** — Decomposes complex user requests into multi-step plans.
- **`prompt_builder.rs`** — Assembles system prompts with personality and tool descriptions.
- **`subagent_registry.rs`** — Tracks spawned subagents for delegation.
- **`acp.rs`** — Agent Control Protocol integration for pause/resume/step/cancel.
- **`transcript.rs`** — Records full conversation transcripts for replay.
- **`compaction.rs`** — Session memory flush and compaction logic.
- **`group.rs`** — Multi-agent group session management.
- **`personality.rs`** — Agent personality and agent registry.
- **`turns.rs`** — Thread and turn management for conversation threading.
- **`artifacts.rs`** — Artifact store for generated files and outputs.
- **`heuristics.rs`** — Desktop/complex task detection keyword heuristics (`is_desktop_task` / `is_complex_task`, EN + ZH keyword lists).

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

```rust
pub enum ProgressEvent {
    Started,
    ToolCalling { name: String, arguments: String },
    ToolResult { name: String, result: String, data: Option<serde_json::Value> },
    ToolResultDelta { name: String, chunk: String, is_error: bool },
    Generating { content: Option<String> },
    ContentDelta { text: String },
    Completed { response: String },
    Error { message: String },
}
```

## Data Flow

1. `process_message()` receives an `IncomingMessage`
2. `build_fresh_context()` calls `MemoryManager::session_context()` to retrieve memories + multimodal references + recent messages
3. Context is injected into the LLM prompt
4. LLM response is parsed; if it contains tool calls, they are executed via `ToolRegistry`
5. Tool results are fed back to the LLM
6. Final response is formatted and returned

## Implemented Features

- Message processing with progress callbacks
- Context building with memory retrieval and multimodal support
- Iteration and token budget tracking with `CostGuard`
- Conversation compression when approaching token limits
- Session persistence and restoration
- Per-session todo list management
- Goal planning with task decomposition
- Subagent spawning and registry tracking
- ACP integration with pause/resume/step/cancel execution control
- Full transcript recording and replay
- Session compaction and memory flush
- Multi-agent group sessions
- Agent personality and template parameter system
- Thread and turn management for conversation threading
- Artifact generation and storage
- Desktop task detection heuristic (`is_desktop_task`)
- Complex task detection heuristic (`is_complex_task`)

