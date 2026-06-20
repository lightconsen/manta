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

- Interval-based task scheduling from `HEARTBEAT.md` (duration strings like `5m`, `30s`, `2h30m`)
- Active hours window (HH:MM format validation)
- Configurable check interval
- Wake priority levels for task scheduling
- Agent wake requests with prompt injection
- Optional response delivery to channels
- Heartbeat event logging
- Integration with Gateway lifecycle
- Config validation for time format

## Scope vs. `cron`

Heartbeat is intentionally **interval-only** — each `HEARTBEAT.md` task fires
on a fixed `Duration` cadence (`is_task_due` compares `last.elapsed()` against
the parsed interval). It does **not** parse cron expressions. Calendar-style
scheduling (5/6-field cron expressions, timezones, one-shot `At`, retry, wake
modes) is handled by the separate [`cron`](cron.md) module via its `Schedule`
enum (`At` / `Every` / `Cron`). Use `cron` when you need cron expressions;
use heartbeat for simple periodic agent wakes within active hours.

