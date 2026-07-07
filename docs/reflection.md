# Reflection Module

LLM self-critique and iterative improvement of agent output. Implements the
Reflection Pattern from *Agentic Design Patterns* (Ch.4).

## Architecture

```
Agent response ──▶ should_trigger() ──no──▶ return as-is
                        │ yes
                        ▼
              ┌─────────────────────┐
              │  ReflectionPipeline │
              │  .reflect()         │
              │                     │
              │  Loop (max 3×):     │
              │    Critic.evaluate  │── pass ──▶ return improved output
              │         │ fail               │
              │         ▼                    │
              │    Critic.improve ───────────┘
              └─────────────────────┘
                        │
                        ▼
              Write reflection_lesson to memory (for cross-turn learning)
                        │
                        ▼
              Return final OutgoingMessage
```

## Files

| File | Purpose |
|------|---------|
| `types.rs` | `Critique`, `QualityCriteria`, `QualityDimension` |
| `config.rs` | `ReflectionConfig`, `ReflectionTrigger` |
| `critic.rs` | LLM judge: `evaluate()` + `improve()` |
| `pipeline.rs` | `ReflectionPipeline` + `ReflectionResult` |
| `mod.rs` | Re-exports |

## Configuration

Enable in `~/.syscity/config.toml`:

```toml
[default_agent.reflection_config]
max_iterations = 3
pass_threshold = 0.7
critic_model = "gpt-4"  # optional; defaults to agent's model

[default_agent.reflection_config.trigger]
adaptive = { min_tokens = 500 }
```

### Trigger Strategies

| Strategy | Behavior |
|----------|----------|
| `adaptive` (default) | Triggers when response length > 200 chars or tool calls were made |
| `always` | Every response is evaluated |
| `after_tool_call` | Only when the response includes tool calls |
| `on_code_generation` | Only when response contains ``` code blocks |

### Quality Criteria (default)

| Dimension | Description |
|-----------|-------------|
| Factual Accuracy | Correctness, no hallucinations |
| Completeness | Fully addresses the request |
| Clarity | Well-structured and readable |
| Instruction Following | Adheres to given instructions |

## Trigger Flow

```
process_message()
  │
  ├── LLM generates response
  ├── PII filtering
  ├── Build OutgoingMessage
  │
  ├── ReflectionPipeline.should_trigger()
  │     └── Checks trigger type + response characteristics
  │
  └── ReflectionPipeline.reflect(content, user_request, tool_results)
        │
        ├── Loop: 0..max_iterations (default 3)
        │     ├── Critic.evaluate() → structured Critique (JSON)
        │     ├── Check: passed all thresholds? → break
        │     └── Critic.improve() → regenerated text
        │
        ├── If iterations > 0:
        │     ├── Replace outgoing content with improved version
        │     └── Write reflection_lesson to MemoryManager
        │           (deduplicated via semantic search)
        │
        └── Return OutgoingMessage
```

## LLM Critic

The critic uses the **same LLM provider** as the agent but with different
system prompts. Temperature is set to 0.0 for deterministic evaluation.

### Evaluate Prompt

System prompt instructs the LLM to act as a quality critic and output
structured JSON:

```json
{
  "dimension_scores": {"Factual Accuracy": 0.85},
  "strengths": ["..."],
  "weaknesses": ["..."],
  "suggested_improvements": ["..."]
}
```

### Improve Prompt

The improve prompt feeds back the original request, flawed response, and
critique weaknesses/suggestions. The LLM regenerates the response addressing
each weakness.

## Cross-Turn Learning

When reflection improves a response (iterations > 0), the critique lesson is
persisted via `MemoryManager::observe()` as type `reflection_lesson`:

- **Content**: summarizes weaknesses and improvement directions (English)
- **Importance**: inversely proportional to initial score (worse = more important)
- **Dedup**: before writing, `MemoryManager::retrieve()` checks for similar
  existing lessons (similarity > 0.45 → skip)

On subsequent turns, the memory system automatically injects relevant lessons
into the agent's context, helping avoid repeated mistakes.

## Error Handling

| Failure | Behavior |
|---------|----------|
| Critic JSON parse fails | Fallback to default score (0.5), does not pass |
| `evaluate()` LLM call fails | Treat as `pass()`, return current content |
| `improve()` LLM call fails | Keep previous iteration's content, exit loop |
| No memory_manager available | Lesson is silently skipped |
| Duplicate lesson | Skipped via dedup check |

## Robustness

- `temperature: 0.0` for deterministic critic evaluation
- Response not blocked if critic is unavailable
- Memory write failures are logged at `warn` level, never propagated
