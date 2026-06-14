# TaskFlow Module

Durable execution for multi-step task plans with checkpoint/resume capabilities.

## Design

TaskFlow provides checkpoint/resume capabilities for long-running task plans. Execution state is persisted to SQLite, allowing recovery after crashes, pauses, or retries.

- **`TaskFlowEngine`** — Main engine for creating and executing task flows
- **`CheckpointStore`** — SQLite-backed persistence for execution state
- **`TaskFlowState`** — Runtime state for a task flow
- **`TaskFlowConfig`** — Configuration for checkpoint intervals and retry policy
- **`TaskExecutor`** — Trait for executing individual tasks
- **`TestExecutor`** — Mock executor for testing

### Architecture

```
TaskPlan
    │
    ▼
TaskFlowEngine::run(flow_id, plan, executor)
    │
    ├──▶ Load checkpoint (if exists)
    ├──▶ Execute next pending task
    ├──▶ Save checkpoint
    ├──▶ Handle failure → retry or abort
    └──▶ Complete → final summary
```

### Checkpointing

- Before each task execution
- After successful task completion
- On failure (for retry/resume)
- Configurable checkpoint interval

## Key Types

```rust
pub struct TaskFlowEngine {
    store: CheckpointStore,
    config: TaskFlowConfig,
}

pub struct TaskFlowConfig {
    pub checkpoint_interval: Duration,
    pub max_retries: u32,
    pub retry_delay: Duration,
}

pub struct TaskFlowState {
    pub flow_id: String,
    pub plan_id: String,
    pub status: TaskFlowStatus,
    pub completed_tasks: Vec<String>,
    pub failed_tasks: Vec<String>,
    pub current_task: Option<String>,
}

pub enum TaskFlowStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
}

pub struct TaskFlowContext {
    pub flow_id: String,
    pub task_id: String,
    pub attempt: u32,
    pub previous_results: HashMap<String, TaskResult>,
}

pub enum TaskResult {
    Success { output: serde_json::Value },
    Failure { error: String },
    Skipped,
}

pub trait TaskExecutor: Send + Sync {
    async fn execute(&self, task: &TaskPlan, ctx: TaskFlowContext) -> TaskResult;
}
```

## Data Flow

```
TaskPlan
    │
    ▼
TaskFlowEngine::run()
    │
    ├──▶ CheckpointStore::load_state()
    ├──▶ Determine next task
    ├──▶ TaskExecutor::execute()
    │       │
    │       └──▶ TaskResult
    │
    ├──▶ CheckpointStore::save_state()
    ├──▶ Handle retry if failed
    └──▶ Continue until complete
```

## Implemented Features

- Durable execution with SQLite checkpointing
- Crash recovery from persisted state
- Pause and resume support
- Configurable retry policy with backoff
- Task result tracking (success, failure, skipped)
- Execution context with previous results
- Task flow summary with completion statistics
- Pluggable task executor trait
- Test executor for unit testing

