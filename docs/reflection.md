# Trajectory Reflection (Retrospect Engine)

LLM self-critique via periodic background review of conversation trajectories.
Implements a Hermes-inspired Retrospect Engine for non-blocking pattern discovery.

## Architecture

```
process_message_with_progress() completes
       │
       ▼
Build OutgoingMessage
       │
       ▼
Increment turn_counter (AtomicU64 on Agent)
       │
       └── [if retrospect_enabled && counter >= min_turns && counter % interval == 0]
             └── tokio::spawn(RetrospectEngine.retrospect())
                   ├── 1. Snapshot last N turns from thread
                   ├── 2. Format as Trajectory string
                   ├── 3. Critic.evaluate_trajectory() → Critique + observation
                   └── 4. Write observation to MemoryManager as "interaction_pattern"
```

The retrospect engine runs **after** the response is returned to the user — it never
blocks the response or modifies output content.

## Files

| File | Purpose |
|------|---------|
| `types.rs` | `Critique`, `QualityCriteria`, `QualityDimension` |
| `config.rs` | `ReflectionConfig`, `RetrospectConfig` |
| `critic.rs` | LLM judge: `evaluate_trajectory()` |
| `retrospect.rs` | `RetrospectEngine` + `RetrospectResult` |
| `trajectory.rs` | `Trajectory`, `TrajectoryStep`, `TrajectoryWindow` |
| `mod.rs` | Re-exports |

## Configuration

Enable in `~/.syscity/config.toml`:

```toml
[default_agent.reflection_config]
retrospect_enabled = true

[default_agent.reflection_config.retrospect]
interval = 10    # fire every N turns (default: 10)
window_size = 5  # review last N turns per fire (default: 5)
min_turns = 3    # minimum turns before first fire (default: 3)
```

To disable:
```toml
[default_agent.reflection_config]
retrospect_enabled = false
```

### Quality Criteria (default)

| Dimension | Description |
|-----------|-------------|
| Factual Accuracy | Correctness, no hallucinations |
| Completeness | Fully addresses the request |
| Clarity | Well-structured and readable |
| Instruction Following | Adheres to given instructions |

Custom criteria can be added via `criteria.dimensions` in the config.

## Retrospect Flow

```
Agent response → turn_counter++
                      │
               counter >= 3 && counter % 10 == 0?
                      │
               ┌──────┴──────┐
               no            yes
               │              │
           return          tokio::spawn {
                               1. Take last 5 turns from thread.turns
                               2. RetrospectEngine.build_trajectory()
                                  → Trajectory { turns: [TrajectoryWindow; 5] }
                               3. trajectory.format_for_prompt()
                                  → "=== CONVERSATION TRAJECTORY ===\n--- Turn 1 ---\n..."
                               4. Critic.evaluate_trajectory(formatted, criteria)
                                  → Critique { observation: Some("...") }
                               5. memory.observe(user_id, observation,
                                   "interaction_pattern", 0.6)
                             }
```

### Trajectory Format

The critic receives:

```
=== CONVERSATION TRAJECTORY (last 5 of 23 turns) ===

--- Turn 1 ---
User: Search for Rust documentation
Assistant: Here's what I found...

--- Turn 2 ---
User: Can you give me an example?
Assistant: Sure, here's a code example...
```

### Trajectory Critic Prompt

The critic evaluates on three dimensions:
- **Tool Usage** — are tools used appropriately and efficiently?
- **Response Quality** — are responses consistent and well-structured?
- **Pattern Recognition** — what themes repeat across turns?

Output includes a critical `observation` field: a single-sentence natural-language
summary of the key interaction pattern.

## Cross-Turn Learning

The retrospect engine writes `"interaction_pattern"` memories to `MemoryManager`:

| Field | Value |
|-------|-------|
| `memory_type` | `"interaction_pattern"` |
| `importance` | `0.6` (fixed) |
| `content` | Natural-language observation |

Example observation:
> "The user tends to ask for code examples then request simplification. Tool usage is efficient — search before edit."

These patterns are automatically injected into future agent context by the
memory system, helping avoid repeated mistakes across sessions.

## Error Handling

| Failure | Behavior |
|---------|----------|
| Critic JSON parse fails | Fallback to default score (0.5), empty observation |
| `evaluate_trajectory()` LLM call fails | Log warning, skip memory write |
| No `memory_manager` available | Memory write silently skipped |
| Retrospect spawn panics | Isolated to background task, response unaffected |

## Robustness

- `temperature: 0.0` for deterministic critic evaluation
- Retrospect runs via `tokio::spawn` — never blocks response
- Memory write failures logged at `warn` level, never propagated
- Retrospect engine is fully optional — disable via `retrospect_enabled = false`
