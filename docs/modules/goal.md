# Goal Module

Goal-based execution for Syscity: the `/goal` command turns a natural-language
goal into structured stop conditions and runs an autonomous sub-agent loop
until all conditions pass or a guardrail trips.

## Design

- **`GoalCondition`** (`condition.rs`) — Deterministic, LLM-free check types
  (`exit_code`, `file_exists`, `numeric`, `pattern`, `static_analysis`). All
  conditions are ANDed.
- **`GoalPlan`** (`plan.rs`) — Parsed `/goal` command: description, ordered
  conditions, `max_rounds` (default 5), optional `model_override`, and the
  `fresh_context` flag (Ralph mode).
- **`GoalRunner`** (`runner.rs`) — Background execution loop: agent acts →
  conditions checked → feedback → repeat.
- **`RoundHandoff`** (`handoff.rs`) — Bounded (≤16K chars), strictly validated
  structured handoff carried between fresh-context rounds.
- **`GoalEvent`** (`event.rs`) — Progress events (`goal.started` →
  `goal.retry` → `goal.check` → `goal.done` | `goal.aborted`) with a
  structured `BlockedReason` on aborts.
- **`GoalStore`** (`persist.rs`) — Per-round checkpoints in
  `~/.syscity/goals/<goal_id>.json`.

### Loop Modes

- **Legacy (default)** — one agent loop driven through the model router, with
  condition feedback as the only cross-round signal.
- **Fresh-context ("Ralph", `GoalPlan::fresh_context`, `/goal --fresh`)** —
  every round runs in a brand-new seedless sub-agent: system prompt + one user
  message, no parent conversation prefix, no session history, no
  personality/memory seeding. The workspace on disk is the long-term memory;
  between rounds only the validated `RoundHandoff` plus the deterministic
  condition results are carried.

### Handoff Contract

In fresh-context mode the round agent must end its final reply with exactly
one fenced block tagged `handoff`:

````text
```handoff
{"status": "continue", "summary": "...", "next_steps": ["..."], "evidence": ["..."]}
```
````

- `status`: `continue` (requires non-empty `next_steps`), `complete`
  (requires non-empty `evidence`; conditions still decide completion), or
  `failed` (stops the loop for human review)
- Blocks over 16,384 characters, missing, malformed, or carrying unknown
  fields are rejected outright — the round fails with `invalid-handoff`,
  never truncated or guessed
- After each validated handoff the runner writes a browsable round note to
  `<workspace>/goals/<goal-id>/round-N.md` (best-effort)

### Suspension and Resume

A goal is *suspended* when it has a persisted checkpoint but no live runner.
Gateway startup deliberately does not re-arm autonomous loops — it logs the
count of suspended goals, and only an explicit `/goal resume <id>` turns a
checkpoint back into a running runner (`src/gateway/goal_spawn.rs`).

Checkpoint retention depends on how the goal ended:

- Done / cancelled / agent error → state file deleted
- Policy stop (`loop-detected`, `max-rounds`, `invalid-handoff`,
  `fatal-config-error`) → checkpoint kept with its structured
  `blocked_reason {code, message}` so the cause survives and the goal can be
  resumed deliberately
- Gateway shutdown → runners are aborted (crash-equivalent), so the last
  round's checkpoint survives and the goal becomes suspended. This is
  deliberately distinct from `/goal cancel`, which deletes the checkpoint —
  shutdown must never discard state a user did not ask to discard

Checkpoint writes are atomic (write `{file}.tmp`, then `rename`): a crash
mid-write leaves the previous checkpoint intact. Runners and their event
relays are registered in the `TaskRegistry` (`goal:{id}` / `goal-relay:{id}`)
so gateway shutdown drains them deterministically.

### Token Accounting and Outcome Write-Back

The runner accumulates executor token spend across its LLM calls. The
cumulative total is persisted in the checkpoint (and continues across
`/goal resume`), attached to `goal.done` / `goal.aborted` events, and
reported in a summary message appended to the parent session thread on every
terminal outcome — so the chat history keeps a durable record of how the goal
ended even after the live WebSocket events are gone. Reporting only: token
budgets are not enforced.

## Key Types

```rust
pub struct GoalPlan {
    pub description: String,
    pub conditions: Vec<GoalCondition>,
    pub max_rounds: usize,              // default 5
    pub model_override: Option<String>,
    pub fresh_context: bool,            // /goal --fresh
}

pub struct RoundHandoff {
    pub status: HandoffStatus,          // Continue | Complete | Failed
    pub summary: String,                // non-empty
    pub next_steps: Vec<String>,        // required for Continue
    pub evidence: Vec<String>,          // required for Complete
}

pub struct BlockedReason {
    pub code: BlockedReasonCode,        // loop-detected | max-rounds | agent-error
                                        // | cancelled | invalid-handoff
                                        // | fatal-config-error
    pub message: String,
}

pub struct PersistedGoalState {
    pub goal_id: String,
    pub parent_session_id: String,
    pub plan: GoalPlan,
    pub round: usize,
    pub condition_history: Vec<PersistedRoundResult>,
    pub blocked_reason: Option<BlockedReason>,  // set on policy stops
    pub last_handoff: Option<RoundHandoff>,     // fresh-context resume state
    pub token_usage: Option<TurnUsage>,         // cumulative executor spend
    // + created_at / updated_at
}
```

## Implemented Features

- LLM translation of goal descriptions into structured check conditions
- Deterministic condition evaluation (no LLM in the check path)
- Loop detection (3 consecutive identical failure signatures)
- Guardrails: max rounds, 25 tool iterations per round, 8 KB tool-result cap
- Fresh-context (Ralph) loop with validated structured handoff between rounds
- Human-readable round notes under `<workspace>/goals/<goal-id>/round-N.md`
- Per-round checkpointing with restart suspension and explicit `/goal resume`
- Atomic checkpoint writes (tmp + rename); runners/relays registered in the
  `TaskRegistry` and drained on shutdown with abort (crash-equivalent)
  semantics
- Structured `blocked_reason` on aborts; policy stops keep their checkpoint
- Per-goal token accounting persisted in checkpoints and reported on terminal
  events (report-only, no enforcement)
- Terminal outcomes written back to the parent session thread
- `goal.progress` gateway events streamed to the originating session
- `syscity goal list/resume/cancel` CLI subcommands (WebSocket transport)
