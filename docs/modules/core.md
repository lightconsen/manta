# Core Module

Domain models, shared business logic, and cross-cutting infrastructure for the Syscity system.

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

1. WebSocket handler creates a `RequestContext`
2. Attaches the context to a tracing span with `trace_id`, `session_id`, `user_id`
3. The async future is `.instrument(span)`-ed so `#[instrument]`-ed functions inherit the fields
4. All `tracing::info!` / `warn!` / `debug!` calls in the call tree carry the context

## Implemented Features

- UUID-based identifiers with parsing and display support
- Atomic engine metrics for operational visibility
- Publish-subscribe event bus for decoupled cross-module communication
- Structured request context with tracing span integration

