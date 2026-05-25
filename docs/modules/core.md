# Core Module

Domain models and shared business logic.

## Design

- **`models.rs`** — Core domain types:
  - `Id` — UUID-based identifier used throughout the system
  - `Timestamp` — Wrapper around `SystemTime`/`DateTime`
  - Common enums and small structs
- **`engine.rs`** — `Engine` trait and implementations for core business logic execution

## Key Types

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Id(pub String);

impl Id {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}
```

## Missing / TODO

- **❌ Missing**: Domain event system for cross-module communication. (Note: memory-specific events exist in `src/memory/events.rs`.)
- **📝 Partial**: Structured logging context propagation — `tracing` with JSON/Pretty/Compact formats and `#[instrument]` attributes are used (`src/utils/logging.rs:1-294`, `src/core/engine.rs`). Missing: explicit context propagation (e.g., `session_id`, `entity_id`) consistently across async boundaries.
- **📝 Partial**: Metrics instrumentation hooks — `ChannelMetrics`/`MetricsManager` (`src/channels/metrics.rs:1-439`) and `Profiler` (`src/utils/profiling.rs:1-610`) exist. Missing: instrumentation hooks inside `src/core/` engine operations.
