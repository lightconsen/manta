# Flow Module

DAG-based workflow execution engine with approval steps and recovery.

## Design

Provides a DAG-based workflow execution engine with support for approval steps, pause/resume, cancellation, and recovery of interrupted flows. Steps are executed in topological order according to their dependency graph.

- **`FlowEngine`** — Main entry point for creating and executing flows
- **`FlowStore`** — Storage trait for persistence (pluggable backend)
- **`InMemoryFlowStore`** — In-memory implementation for testing
- **`Flow`** — Flow definition with DAG structure
- **`FlowStep`** — Individual step with tool invocation, dependencies, and approval requirements
- **`StepExecutionState`** — Runtime state tracking for each step

### Flow Status Lifecycle

```
Pending ──▶ Running ──▶ Completed
   │           │
   ▼           ▼
Paused     Failed
   │           │
   ▼           ▼
Cancelled  (terminal)
```

### Step Status Lifecycle

```
Pending ──▶ Running ──▶ Succeeded
   │           │
   ▼           ▼
WaitingApproval ──▶ Approved ──▶ Running
   │
   ▼
Rejected
```

## Key Types

```rust
pub struct FlowEngine {
    store: Arc<dyn FlowStore>,
}

pub struct Flow {
    pub id: FlowId,
    pub name: String,
    pub description: String,
    pub steps: Vec<FlowStep>,
}

pub struct FlowStep {
    pub id: FlowStepId,
    pub name: String,
    pub description: String,
    pub tool_name: String,
    pub tool_args: serde_json::Value,
    pub depends_on: Vec<FlowStepId>,
    pub approval: ApprovalRequirement,
    pub timeout_secs: u64,
    pub retry_count: u32,
    pub on_failure: FailureAction,
}

pub enum ApprovalRequirement {
    Never,
    Always,
    AfterAll,
}

pub enum FailureAction {
    Abort,
    Skip,
    Retry,
    Continue,
}

pub enum FlowStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

pub enum StepStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    WaitingApproval,
    Approved,
    Rejected,
}
```

## Data Flow

```
Flow Definition
    │
    ▼
FlowEngine::create_flow()
    │
    ▼
FlowEngine::execute_flow()
    │
    ├──▶ Topological sort of steps
    ├──▶ Check approval requirements
    ├──▶ Execute tool with timeout
    ├──▶ Handle failure (Abort/Skip/Retry/Continue)
    └──▶ Update step state
            │
            ▼
        FlowStore::save_state()
```

## Implemented Features

- DAG-based workflow execution with topological ordering
- Human approval gates (Never, Always, AfterAll prerequisites)
- Failure handling strategies (Abort, Skip, Retry, Continue)
- Pause/resume and cancellation support
- Step timeout and retry configuration
- Pluggable persistence via `FlowStore` trait
- In-memory store for testing
- Full runtime state tracking per step
- Recovery of interrupted flows from persisted state

