# ACP Module

Agent Control Plane (ACP) — subagent spawning, session management, and runtime execution control.

## Design

ACP provides a control plane for managing agent execution with pause/resume/step/cancel capabilities and subagent spawning with thread binding.

- **`AcpControlPlane`** — Central control plane managing subagents, threads, and sessions
- **`ExecutionController`** — Pause/resume/step/cancel state machine inserted into the agent's tool-call loop
- **Session Actor Loop** — Per-session serial message queue (`mpsc::channel`) with `ExecutionController` coordination
- **ACP Actor Loop** — Routes commands to the appropriate session actor

### Execution Modes

| Mode | Behavior |
|------|----------|
| `Run` | One-shot execution, context discarded after completion |
| `Session` | Persistent session, context kept across turns |

### Runtime States

```
Idle ──▶ Running ──▶ Paused ◀──▶ Stepping
  │        │           │
  ▼        ▼           ▼
Cancelled (terminal)
```

### Thread Binding

- `New` — Isolated thread for the subagent
- `Parent` — Inherit parent's thread
- `Thread(id)` — Bind to a specific thread
- `Auto` — Automatic based on context

## Key Types

```rust
pub struct AcpControlPlane {
    subagents: Arc<RwLock<HashMap<String, SubagentHandle>>>,
    threads: Arc<RwLock<HashMap<String, ThreadContext>>>,
    sessions: Arc<RwLock<HashMap<AcpSessionId, AcpSession>>>,
    command_tx: mpsc::Sender<AcpCommand>,
    max_iterations: usize,
}

pub struct ExecutionController {
    state: RwLock<RuntimeState>,
    notify: tokio::sync::Notify,
}

pub enum AcpCommand {
    ExecuteSession { agent, message, respond_to },
    ExecuteRun { agent, message, respond_to },
    ExecuteSessionWithProgress { agent, message, progress_cb, respond_to },
    Pause { session_id },
    Resume { session_id },
    Step { session_id },
    Cancel { session_id },
    GetStatus { session_id, respond_to },
    Shutdown,
}
```

## Data Flow

```
Gateway ──▶ AcpControlPlane ──▶ ACP Actor Loop
                                   │
                                   ├──▶ Session Actor (serial queue)
                                   │      └──▶ Agent::process_message()
                                   │            └──▶ ExecutionController::check_and_wait()
                                   └──▶ Session Actor 2
```

## Integration

- `AcpSessionTool` and `AcpSpawnTool` in `tools/` expose ACP to the agent
- `Agent::process_message_with_controller()` checks `ExecutionController` between iterations
- `SubagentHandle` provides `command_tx` for parent-child communication
