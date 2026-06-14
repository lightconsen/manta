# Standing Orders Module

Persistent background agent programs that run on a cron schedule.

## Design

Standing orders are cron-scheduled jobs that periodically send a prompt to a target agent and optionally dispatch the response to a channel. They follow the same lifecycle pattern as `DreamScheduler` in `src/memory/dreaming.rs` and borrow the agent-wake pattern from `src/heartbeat/runner.rs`.

- **`StandingOrderManager`** — Manages a collection of standing order background tasks
- **`StandingOrderConfig`** — Configuration with enablement and order list
- **`StandingOrder`** — Individual order definition

### Order Definition

```rust
pub struct StandingOrder {
    pub name: String,
    pub description: Option<String>,
    pub agent_id: String,
    pub schedule: String,           // Cron expression
    pub prompt: String,
    pub output_channel: Option<String>,
    pub enabled: bool,
    pub timeout_secs: Option<u64>,
}
```

### Architecture

```
StandingOrderConfig
    │
    ├──▶ enabled = false → skip all
    └──▶ enabled = true
            │
            ▼
        StandingOrderManager::start()
            │
            ├──▶ For each enabled order:
            │       │
            │       ├──▶ Parse cron expression
            │       ├──▶ Spawn tokio task
            │       ├──▶ Sleep until next tick
            │       ├──▶ Fire prompt to agent
            │       ├──▶ Optionally dispatch to channel
            │       └──▶ Handle shutdown signal
            │
            └──▶ Track shutdown senders
```

## Key Types

```rust
pub struct StandingOrderManager {
    config: StandingOrderConfig,
    state: Arc<GatewayState>,
    shutdown_txs: Vec<(String, mpsc::Sender<()>)>,
}

pub struct StandingOrderConfig {
    pub enabled: bool,
    pub orders: Vec<StandingOrder>,
}

pub struct StandingOrder {
    pub name: String,
    pub description: Option<String>,
    pub agent_id: String,
    pub schedule: String,
    pub prompt: String,
    pub output_channel: Option<String>,
    pub enabled: bool,
    pub timeout_secs: Option<u64>,
}
```

## Data Flow

```
Cron Tick
    │
    ▼
IncomingMessage::new("system", session_id, prompt)
    │
    ▼
Agent::process_message()
    │
    ├──▶ Ok(response) ──▶ Optional channel dispatch
    ├──▶ Err(e) ──▶ Error log
    └──▶ Timeout ──▶ Warning log
```

## Implemented Features

- Cron-scheduled background agent execution
- Per-order agent targeting
- Optional response dispatch to channels
- Configurable timeout per order
- Graceful shutdown with signal-based cancellation
- Invalid cron expression handling (warn and skip)
- Agent existence validation
- Integration with Gateway state and reply dispatcher
- Config-driven enablement (global and per-order)

