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

## Missing / TODO

- **✅ Implemented**: Full ACP protocol handlers — `ExecutionController` with RuntimeState machine is fully implemented. WebSocket ACP RPC handlers exist for spawn/terminate/message/status/pause/resume/step/cancel/tree/execute. ACP lifecycle events (`acp.spawned`, `acp.completed`, `acp.status_changed`, `acp.recovered`, `acp.thread_switched`) are emitted from `AcpControlPlane`, broadcast via `GatewayEvent`, and mapped to protocol `event` frames in `gateway_event_to_ws`. See `src/acp/mod.rs`, `src/gateway/mod.rs`, `src/gateway/protocol.rs`.
- **✅ Implemented**: Session store persistence — `AcpControlPlane` always receives the `SessionStore` when SQLite persistence is available, logs whether persistence is active, and warns when database storage is requested but unavailable. `load_persisted_sessions()` validates and restores persisted sessions on startup. See `src/acp/mod.rs`, `src/gateway/mod.rs`.
- **✅ Implemented**: Subagent registry routing — `DelegateTool::spawn_child` now uses `SubagentRegistry::spawn` as the authority for depth/concurrency limits and unifies the local child id with the registry run id. `cancel` correctly kills the registry run, and status/list responses report registry-backed limits. See `src/tools/delegate_tool.rs`, `src/agent/subagent_registry.rs`.
- **✅ Implemented**: Subagent crash recovery — the watchdog task detects panics, marks `SubagentStatus::Crashed`, and automatically restarts crashed subagents with exponential backoff. Recovery is controlled by `CrashRecoveryConfig` and per-subagent `retry_on_crash` / `max_crash_retries`. See `src/acp/mod.rs:1290-1370`, `src/acp/mod.rs:1515-1600`.
- **✅ Implemented**: Thread context switching and migration between threads — `AcpControlPlane` provides `switch_thread_active_subagent()` for context switches and `migrate_subagent_thread()` to move a subagent (and its queued thread messages) to another thread. See `src/acp/mod.rs:1670-1900`.
- **✅ Implemented**: Cross-session subagent communication beyond parent-child — `AcpBus` provides pub/sub messaging across unrelated ACP sessions via topics. `AcpControlPlane` exposes `bus_subscribe`, `bus_unsubscribe`, `bus_publish`, `bus_poll`, `bus_poll_all`, `bus_topics`, and `bus_subscribers`. See `src/acp/mod.rs:820-930`, `src/acp/mod.rs:1900-2000`.
