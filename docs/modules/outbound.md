# Outbound Module

Outbound pipeline for processing agent outputs before delivery to users.

## Design

The outbound pipeline handles everything that happens *after* the agent produces a response:

```
Agent Output
-> Trajectory (capture execution trace)
-> Canvas (render dynamic UI)
-> SSE (stream to connected clients)
-> Reply Dispatcher (route to correct channel)
-> Side Effects (memory, cron, webhooks, …)
```

- **`trajectory.rs`** — `TrajectoryLog` and `TrajectoryWriter` for execution trace persistence
- **`canvas.rs`** — Canvas rendering for A2UI components in agent output
- **`sse.rs`** — `SseStreamer` for Server-Sent Events to connected clients
- **`reply_dispatcher.rs`** — `ReplyDispatcher` for routing messages to the correct channel
- **`side_effects.rs`** — `SideEffectExecutor` for post-delivery actions (memory storage, cron, webhooks)

### Pipeline Stages

| Stage | File | Purpose |
|-------|------|---------|
| 1 | Trajectory | Persist execution trace |
| 2 | Canvas | Detect and render A2UI components |
| 3 | SSE | Stream tool call events to clients |
| 4 | Reply Prefix | Prepend model info / metadata |
| 5 | Reply Dispatch | Route to correct channel |
| 6 | Side Effects | Execute post-delivery actions |

## Key Types

```rust
pub struct OutboundResult {
    pub text: String,
    pub canvas_update: Option<CanvasUpdate>,
    pub sse_events: Vec<SseEvent>,
    pub side_effects: Vec<SideEffect>,
    pub session_id: String,
    pub channel: String,
}

pub trait OutboundPipeline: Send + Sync {
    async fn process(&self, ctx: OutboundContext) -> OutboundResult;
}

pub struct OutboundContext {
    pub session_id: String,
    pub channel: String,
    pub agent_id: String,
    pub raw_output: String,
    pub tool_calls: Vec<ToolCall>,
    pub trajectory: TrajectoryLog,
    pub usage: Option<Usage>,
}

pub enum SseEvent {
    ToolStart { name: String },
    ToolComplete { name: String, result: String },
    ContentDelta { text: String },
    Done,
    Error { message: String },
}

pub struct TrajectoryEntry {
    pub timestamp: DateTime<Utc>,
    pub action: String,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub duration_ms: u64,
}
```

## Data Flow

```
Agent Output
    │
    ▼
DefaultOutboundPipeline::process()
    │
    ├──▶ TrajectoryWriter::append_log()
    ├──▶ Canvas component detection (JSON parse)
    ├──▶ SSE streaming (tool start/complete events)
    ├──▶ Reply prefix application
    ├──▶ ReplyDispatcher::dispatch()
    └──▶ SideEffectExecutor::execute_batch()
            │
            ▼
        OutboundResult
```

## Implemented Features

- Trajectory persistence for execution trace logging
- Canvas component detection and rendering in agent output
- SSE streaming for real-time client updates
- Reply prefix engine with template support
- Reply dispatch to correct channel
- Side effect execution for post-delivery actions
- Token usage tracking in outbound context
- Tool call event streaming
- Fire-and-forget trajectory persistence

