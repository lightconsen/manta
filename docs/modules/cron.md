# Cron Module

Cron scheduler for Syscity with production-grade scheduled task execution.

## Design

- **`CronScheduler`** (`cron.rs`) — Primary scheduler used by Gateway and CronTool
- **`CronJob`** — Job definition with schedule expression, target agent, delivery channel, and retry policy
- **`CronTool`** (`crate::tools::cron_tool`) — AI-facing tool interface for managing cron jobs

### Architecture

```
CronExpression ──▶ CronScheduler::check()
                      │
                      ├──▶ Match found ──▶ spawn_job()
                      │       │
                      │       └──▶ Agent::process_message()
                      │               │
                      │               └──▶ Channel::send() (if output_channel set)
                      │
                      └──▶ No match ──▶ sleep until next check
```

### Job Definition

```rust
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub schedule: String,          // Cron expression
    pub agent_id: String,          // Target agent
    pub prompt: String,            // Prompt to send
    pub output_channel: Option<String>, // Optional channel for response
    pub enabled: bool,
    pub retry_count: u32,
    pub timeout_secs: u64,
}
```

## Key Types

```rust
pub struct CronScheduler {
    jobs: Vec<CronJob>,
    check_interval: Duration,
    last_run: HashMap<String, DateTime<Utc>>,
}

pub struct CronJob {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub agent_id: String,
    pub prompt: String,
    pub output_channel: Option<String>,
    pub enabled: bool,
    pub retry_count: u32,
    pub timeout_secs: u64,
}
```

## Implemented Features

- Cron expression-based scheduling
- Per-job agent targeting
- Optional response delivery to channels
- Retry logic with configurable count
- Timeout protection for job execution
- Run history tracking
- AI-facing tool for dynamic job management
- Integration with Gateway lifecycle

