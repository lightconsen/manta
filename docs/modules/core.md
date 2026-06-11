# Core Module

Domain models and shared business logic.

## Design

- **`models.rs`** — Core domain types:
  - `Id` — UUID-based identifier used throughout the system
  - `Timestamp` — Wrapper around `SystemTime`/`DateTime`
  - Common enums and small structs
- **`engine.rs`** — `Engine` trait and implementations for core business logic execution
  - `EngineMetrics` — Atomic counters (entities created/deleted/updated, errors, archive runs)
- **`events.rs`** — Domain event system for cross-module communication
  - `EventBus` — Publish-subscribe channel
  - `CoreEvent` enum — `EntityCreated`, `EntityUpdated`, `EntityDeleted`, `EntityArchived`
  - `EventHandler` trait — Consumers implement this to react to events
- **`context.rs`** — Structured request context for tracing
  - `RequestContext` — Holds `trace_id`, `session_id`, `user_id`, `entity_id`
  - `attach_to_span()` — Creates a tracing span with these fields, injected at WebSocket entry points

## Key Types

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Id(pub Uuid);

impl Id {
    pub fn new() -> Self { Self(Uuid::new_v4()) }
    pub fn parse(s: &str) -> crate::Result<Self> { ... }
}
```

## Data Flow

### Domain Events

1. `Engine::create_entity()` completes a mutation
2. Calls `self.emit_event(CoreEvent::entity_created(id, name))`
3. `EventBus` fans out the event to all registered `EventHandler`s
4. Handlers react (e.g., log, update memory, trigger side-effects)

### Tracing Context

1. WebSocket handler (`handle_acp_execute_session`, `handle_acp_execute_run`) creates a `RequestContext`
2. Attaches the context to a tracing span with `trace_id`, `session_id`, `user_id`
3. The async future is `.instrument(span)`-ed so `#[instrument]`-ed functions inherit the fields
4. All `tracing::info!` / `warn!` / `debug!` calls in the call tree carry the context

## Missing / TODO

- **✅ Implemented**: Domain event system — `EventBus` with `CoreEvent` enum, `EventHandler` trait, `subscribe`/`publish`/`unsubscribe`. Used by `Engine` for entity mutations. See `src/core/events.rs`.
- **✅ Implemented**: Structured logging context propagation — `RequestContext` with `attach_to_span()`, injected at WebSocket entry points (`handle_acp_execute_session`, `handle_acp_execute_run`). Fields propagate to all `#[instrument]`-ed functions. See `src/core/context.rs`, `src/gateway/ws.rs`.
- **✅ Implemented**: Metrics instrumentation hooks — `EngineMetrics` with atomic counters wired into all `Engine` methods (create/update/delete/archive). Accessible via `engine.metrics()`. See `src/core/engine.rs`.
- **✅ Implemented**: Event persistence — `EventLog` with JSONL-backed append/read/read_by_type. See `src/core/events.rs`.
- **✅ Implemented**: Metrics export — `syscity_engine_*` gauges exposed via `/api/v1/metrics` (entities_created/updated/deleted, errors_total, archive_runs_total, entities_archived_total). See `src/gateway/handlers/health.rs`.
