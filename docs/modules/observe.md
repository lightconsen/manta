# Observe Module

Per-turn observability: records, collection, persistence, aggregation, and retention for every agent turn (Duration / Turns / Calls).

## Design

Every agent turn produces a full-fidelity **`TurnRecord`**, written as one JSON file per turn under `~/.syscity/turns/YYYY-MM-DD/<turn_id>.json` (local-time date partition, atomic tmp-file + rename). In parallel, the aggregateable numbers land as SQLite rows in `~/.syscity/data/syscity.db` (see `agent::session_store::metrics`):

- **`llm_calls`** — one row per LLM call (provider, model, duration, TTFT, token usage incl. cache read/creation, finish reason, error)
- **`tool_call_metrics`** — one row per tool call (name, duration, success, error)
- **`turn_outcomes`** — one row per turn (queue wait, duration, TTFT, round/call counts, cache hit, end state)

- **`TurnMetricsCollector`** (`collector.rs`) — Owned by a single turn in `agent_engine`; accumulates queue wait, reasoning/text deltas, LLM rounds, tool calls, token usage, and cache hits. Closed explicitly via `finish` / `fail` / `abort`; if the turn future is dropped without a terminal call, `Drop` persists an `aborted` record best-effort (blocking write, no async context).
- **`TurnRecord`** and friends (`record.rs`) — The persisted schema (`SCHEMA_VERSION = 1`). Free-text fields (args, results, previews, errors) are truncated to `MAX_FIELD_BYTES` (4096) at persistence time via `TurnRecord::finalize`.
- **`TurnMetricsWriter`** (`writer.rs`) — Atomic JSON persistence; async `write` plus a blocking variant backing the `Drop` fallback.
- **`TurnMetricsSink`** (`mod.rs`) — Optional numeric sink; when attached, the collector additionally persists the SQLite metric rows. The JSON writer always runs.
- **`aggregate`** (`aggregate.rs`) — Pure aggregation functions over persisted records (percentiles, Duration / Turns / Calls stats blocks) used by the CLI.
- **`prune`** (`prune.rs`) — Retention sweeps shared by `syscity observe prune` and the daemon-startup auto-cleanup.

### Agent Attribution

Each record is tagged with the stable `agent_id` set at agent spawn (`TurnContext.agent_id` → `TurnRecord.agent_id`, serde-defaulted for backward compatibility with old records). Subagent turns are tagged with the subagent id (see `acp.md`); bridge agents keep an empty agent field.

### Failure Telemetry

Record write failures increment `WRITE_FAILURES`, surfaced as the `syscity_observe_write_failures_total` counter at `/api/v1/metrics`. Observability is best-effort: a write failure never fails the turn.

## CLI

```
syscity observe stats  [--today] [--days N] [--agent ID] [--json]
syscity observe list   [--session ID] [--agent ID] [--errors] [--days N] [--limit N]
syscity observe show   <turn_id> [--json]
syscity observe export <turn_id> [--md] [--context N]
syscity observe prune  [--older-than 30d]
```

- `stats` — aggregate Duration / Turns / Calls blocks; DB-first with JSON fallback when the store is unavailable
- `list` — recent turn records (AGENT column; `--errors` shows only error/abort outcomes)
- `show` — a single full turn record
- `export` — a self-contained analysis bundle (full untruncated messages + system prompt, optionally the last N prior session messages for context) for feeding an AI
- `prune` — delete JSON date directories and metric rows older than N days (default 30)

## Retention

The daemon sweeps old observability data at startup, governed by `observe.retention_days` in the gateway config (default 30; `0` disables auto-cleanup). JSON date directories are removed by lexicographic date comparison (`YYYY-MM-DD` names sort correctly); metric rows are removed by `delete_metrics_before` against a local-midnight epoch-ms cutoff. `syscity observe prune --older-than` overrides the configured value for a one-off run.

## Key Types

```rust
pub struct TurnRecord {
    pub schema_version: u32,
    pub turn_id: String,
    pub session_id: Option<String>,
    pub conversation_id: String,
    pub agent_id: String,          // stable agent that handled the turn
    pub thread_id: String,
    pub turn_index: u32,
    pub state: TurnEndState,       // Complete | Error | Aborted
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
    pub ttft_ms: Option<u64>,
    pub model: String,
    pub queue_wait_ms: Option<u64>,
    pub cache_hit: bool,
    pub error: Option<ObservedError>,
    pub usage: ObservedUsage,      // incl. cache read/creation tokens
    pub llm_rounds: Vec<LlmRoundRecord>,
    pub tool_calls: Vec<ObservedToolCall>,
    // plus truncated text previews of the user/assistant/reasoning content
}

pub trait TurnMetricsSink: Send + Sync {
    fn persist_turn<'a>(&'a self, rec: &'a TurnRecord)
        -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}
```

## Implemented Features

- One JSON record per turn under `~/.syscity/turns/YYYY-MM-DD/`, atomic writes
- SQLite metric rows (`llm_calls`, `tool_call_metrics`, `turn_outcomes`) for aggregation
- Duration, TTFT, queue-wait, round/call counts, token usage (incl. cache hit), error and abort tracking
- `Drop`-fallback `aborted` record for turns killed mid-flight
- Per-record agent attribution (incl. subagent ids)
- 4096-byte truncation of free-text fields at persistence time
- `syscity observe {stats,list,show,export,prune}` CLI with `--agent` filtering
- Daemon-startup retention sweep (`observe.retention_days`) plus manual prune
- `syscity_observe_write_failures_total` counter at `/api/v1/metrics`
