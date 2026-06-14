# Heartbeat Module

Heartbeat scheduler for periodic agent wake and health checks.

## Design

- **`HeartbeatConfig`** — Configuration for heartbeat scheduling
- **`HeartbeatRunner`** — Main runner that schedules and executes heartbeats
- **`HeartbeatEvent`** — Events emitted during heartbeat execution
- **`WakeRequest`** — Request to wake an agent for a periodic task
- **`WakePriority`** — Priority levels for wake requests

### Architecture

```
Config::heartbeat
    │
    ├──▶ enabled
    ├──▶ active_hours_start / active_hours_end
    ├──▶ check_interval_seconds
    └──▶ tasks
            │
            ▼
        HeartbeatRunner::start()
            │
            ├──▶ Sleep until next check interval
            ├──▶ Check active hours
            ├──▶ Evaluate task conditions
            └──▶ Emit WakeRequest
                    │
                    ▼
                Agent::process_message()
```

## Key Types

```rust
pub struct HeartbeatConfig {
    pub enabled: bool,
    pub active_hours_start: String,  // HH:MM format
    pub active_hours_end: String,    // HH:MM format
    pub check_interval_seconds: u64,
}

pub struct HeartbeatRunner {
    config: HeartbeatConfig,
    state: Arc<GatewayState>,
}

pub enum HeartbeatEvent {
    Started,
    TaskFired { task_name: String },
    TaskCompleted { task_name: String, result: String },
    TaskFailed { task_name: String, error: String },
    Stopped,
}

pub struct WakeRequest {
    pub agent_id: String,
    pub prompt: String,
    pub priority: WakePriority,
    pub channel: Option<String>,
}

pub enum WakePriority {
    Low,
    Normal,
    High,
    Critical,
}
```

## Implemented Features

- Configurable heartbeat scheduling with cron-like expressions
- Active hours window (HH:MM format validation)
- Configurable check interval
- Wake priority levels for task scheduling
- Agent wake requests with prompt injection
- Optional response delivery to channels
- Heartbeat event logging
- Integration with Gateway lifecycle
- Config validation for time format

