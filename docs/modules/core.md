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

- **Missing**: Domain event system for cross-module communication.
- **Missing**: Structured logging context propagation through the engine.
- **Missing**: Metrics instrumentation hooks.
