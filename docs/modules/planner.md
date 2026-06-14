# Planner Module

Goal planner for decomposing high-level goals into executable task DAGs.

## Design

The planner takes a user goal (e.g. "deploy this project to a server"), breaks it into a directed acyclic graph of `Task`s, and executes them in topological order with automatic verification and rollback.

- **`GoalDecomposer`** — Breaks goals into subtasks using LLM or rule-based decomposition
- **`DagScheduler`** — Topological execution of task dependencies
- **`TaskExecutor`** — Executes individual tasks with retry and verification
- **`VerificationEngine`** — Checks task success against criteria
- **`RollbackManager`** — Snapshot-based state restoration on failure
- **`ErrorDiagnosisEngine`** — Root cause analysis and remediation suggestions
- **`ToolLearningEngine`** — Learns from successful tool executions for future suggestions
- **`WorkflowRecorder`** / **`WorkflowPlayer`** — Record and replay task sequences
- **`PersistentTaskManager`** — SQLite-backed durable task queue
- **`TaskScheduler`** — Scheduled task execution with cron support
- **`CompositeToolRegistry`** — Registry for multi-step composite tools

### Task Lifecycle

```
Pending ──▶ Running ──▶ Completed
   │           │
   ▼           ▼
Failed    RolledBack
```

## Key Types

```rust
pub struct Task {
    pub id: TaskId,
    pub description: String,
    pub action: DesktopAction,
    pub dependencies: Vec<TaskId>,
    pub verification: Option<VerificationCriteria>,
    pub snapshot_before: bool,
    pub max_retries: u32,
    pub retry_delay: Duration,
    pub status: TaskStatus,
    pub error: Option<String>,
    pub result: Option<String>,
}

pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    RolledBack,
}

pub struct Plan {
    pub goal: String,
    pub tasks: HashMap<TaskId, Task>,
}

pub struct VerificationCriteria {
    pub expected_output: Option<String>,
    pub expected_files: Vec<String>,
    pub timeout: Duration,
}

pub struct ErrorDiagnosisEngine {
    // Root cause analysis and remediation
}

pub struct ToolLearningEngine {
    // Experience-based tool suggestion
}
```

## Data Flow

```
User Goal
    │
    ▼
GoalDecomposer::decompose()
    │
    ▼
Plan (DAG of Tasks)
    │
    ▼
DagScheduler::execute()
    │
    ├──▶ Topological sort
    ├──▶ Execute ready tasks
    ├──▶ Verify results
    ├──▶ Retry on failure
    └──▶ Rollback if needed
```

## Implemented Features

- Goal decomposition into task DAGs
- Topological task scheduling with dependency resolution
- Task execution with retry and timeout
- Verification engine with multiple criteria types
- Rollback manager with snapshot-based restoration
- Error diagnosis with root cause analysis
- Tool learning from execution history
- Workflow recording and playback
- Persistent task queue with SQLite backend
- Scheduled task execution
- Composite tool registry for reusable multi-step tools
- Task state machine (Pending, Running, Completed, Failed, RolledBack)

