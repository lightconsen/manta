# Module Review Criteria

Code review follows a size-tiered process. The criteria are designed to cover key risks while avoiding over-scrutiny of small modules.

---

## Quick Checklist (tick as you review)

High-impact items for all module sizes. Scan in ~2 minutes:

```
[ ] Is the core logic correct? Does the average calculation, empty-input behavior match expectations? (correctness)
[ ] Can the module's responsibility be stated in one sentence? (architecture)
[ ] Are errors silently discarded via let _ / .ok() on critical paths? (error handling)
[ ] Does std::sync::Mutex ever cross an .await boundary? (deadlock)
[ ] Are all pub types/functions necessary? Anything that should be pub(crate)? (minimal surface)
[ ] Can boundary conditions cause a panic? Empty vec, unwrap, divide-by-zero (panic safety)
[ ] Do long-running tasks have a shutdown signal? (select! + shutdown)
[ ] Do tests cover empty/error/edge cases? (test coverage)
```

If all 8 pass **and** the module is < 150 lines, stop here. Otherwise proceed to detailed review.

> Note: this checklist verifies "is the code correct?", not "is the inter-module contract correct?".
> Cross-module coupling and accidental shared-state leaks belong to design review, not this document.

### Verification: what "actually checked" looks like

The essence of rubber-stamping is: **reading conclusions, not evidence**. Here is the difference between real and fake checking for each item:

| Item | Fake (rubber-stamp) | Real |
|------|-------------------|------|
| Correctness | "Looks right" | Mentally trace at least one path: what happens on empty input? Does the loop boundary overflow? |
| Architecture | "Structure is clear" | Trace one data flow: input → processing → output, confirm each step belongs in this module |
| Error handling | "No let _ found" | `grep -n 'let _ ='` or `rg '\.ok\(\)'` — verify each occurrence has a log or fallback |
| Deadlock | "No cross-await locks" | `grep -n 'std::sync::Mutex'` — check every `lock()` call for `.await` in its async context |
| Minimal surface | "Not many pub items" | Read every `pub fn` and `pub struct` — ask "does an external caller really need this?" |
| Panic safety | "No unwrap found" | `grep -n 'unwrap\|expect\|\[.*\]'` — verify each is within a controlled scope |
| Shutdown safety | "Has shutdown" | Find every `select!` branch — confirm the `shutdown` branch exits the loop, not continues |
| Test coverage | "Tests exist" | Read the test cases — what does each assertion check? What arguments are passed for edge cases? |

> Core principle: **if you can't say in one sentence what specific evidence you examined for an item, you haven't really checked it.**

### Red flags for rubber-stamping

These signals indicate the review is drifting into formality:

- **"Looks fine"** appears across 3+ items → skimming, not reviewing
- **Only quotes line numbers** without the surrounding context → reading locally, not understanding globally
- **Every item is checked** with zero questions raised → zero findings usually means not looking hard enough
- **Review time significantly below target** → small < 5 min, medium < 15 min, large < 30 min suggests ticking boxes

---

## Review Dimensions

8 dimensions covering merged overlapping areas.

### 1. Architecture & API Surface

| Item | Description |
|------|-------------|
| Separation of concerns | Is the module boundary clear? Does it take on too many responsibilities? |
| Dependency direction | Does it depend on modules it shouldn't know about? Any circular deps? |
| Minimal public surface | Are there types/functions/fields exposed as `pub` that should be `pub(crate)`? |
| File organization | Single file > 500 lines? `mod.rs` crammed with too many types? |
| Naming conventions | Do names follow the project conventions? |
| Invariant declaration | Does the module register its data invariants with `core::invariants`, or carry an explicit `INVARIANTS-NONE:` marker justifying why it has none? (declare-or-register convention; enforced by `scripts/static-analysis.sh --full`, see `src/core/invariants.rs`) |

### 2. Thread Safety & State Management

| Item | Description |
|------|-------------|
| Lock type choice | Is `std::sync::Mutex` held across `.await`? (deadlock risk) |
| Lock granularity | Single coarse lock instead of finer-grained locks? Read path could use `RwLock`? |
| Task lifecycle | Are `tokio::spawn` handles registered in `TaskRegistry`? |
| Shutdown safety | Do long-running loops use `select!` with a shutdown signal? |
| Cache staleness | Is a cached value used for freshness checks without re-validation? |
| Reset consistency | When a reset path exists, are all internal states reset consistently? |

### 3. Error Handling & Observability

| Item | Description |
|------|-------------|
| Silent discards | Are errors silently dropped via `let _ =` or `.ok()`? |
| Error types | Proper `thiserror` enum or `anyhow` context? |
| Panic safety | Does a single source failure crash the entire module? |
| Degradation | When dependencies fail, is the behavior reasonable? |
| Log coverage | Are non-fatal failures logged with `warn!`/`error!` with useful context? |
| Log levels | Are high-frequency events using `debug!` instead of `info!`? |

### 4. Documentation Quality

| Item | Description |
|------|-------------|
| Public API docs | Do all `pub` items have `///` docs? |
| Module docs | Does `//!` explain purpose and typical usage (not just what it does)? |
| Docs vs reality | Are type/function names in docs outdated? |

### 5. Configuration & Magic Numbers

| Item | Description |
|------|-------------|
| Hardcoded constants | Thresholds/timeouts/capacities that should be configurable? |
| Config validation | Is config validated at construction (not silently using invalid values at runtime)? |
| Default value docs | Are defaults documented with their rationale? |

### 6. Security

| Item | Description |
|------|-------------|
| Input validation | Is external data sanitized before being trusted? |
| Path traversal | Do file path operations check for `..`? |
| Command injection | Does `std::process::Command` pass unsanitized input? |
| Sensitive data leaks | Could passwords/tokens/keys appear in logs? |

### 7. Performance & Resources

| Item | Description |
|------|-------------|
| Unnecessary allocations | `String` or `Vec` allocated on hot path? (e.g., `format!` just for sorting) |
| I/O patterns | Batch vs line-by-line? N+1 queries? |
| Memory leaks | Unbounded channels or collections that can accumulate? |
| Unnecessary clones | Redundant `.clone()` or deep copies on hot paths? |

### 8. Test Coverage & Compatibility

| Item | Description |
|------|-------------|
| Happy path | Standard operation flow covered? |
| Edge cases | Empty input, limit values, boundary conditions? |
| Error paths | Corrupted data, timeouts, permission denied? |
| Regression tests | Are fixed bugs accompanied by corresponding tests? |
| Breaking changes | Does a change break pub API? Should `#[deprecated]` be added? |
| Serialization compat | If serialization format changes, are old and new versions compatible? |

---

## Code Smell Checklist

Not a numbered dimension; keep in mind during review. Severity depends on how hot the path is.

### High
- Single function > 80 lines
- Same fix pattern appearing across reviews (should add a pre-commit rule)
- `todo!()` or `unreachable!()` in non-test code

### Medium
- Declared but unused `_` variables (concern for internal code; pub API is fine)
- 3+ consecutive `if let` / `match`

### Low
- 2+ consecutive `unwrap()` calls (fine in test code)
- `format!(..)` only used for sorting

---

## Ancillary Checklist

Not a numbered dimension; check opportunistically in relevant reviews.

- **Dependency management**: unnecessary deps? Feature flags overgrown? `cargo audit` findings?
- **Send + Sync**: Do trait objects and closures auto-implement these where needed?
- **Config propagation**: Do new config fields have sensible defaults to avoid breaking existing users?
- **Registered invariants**: If the module registers checks with `core::invariants`, does each check actually verify what its description claims, and does `syscity invariants` still pass after the change?

---

## Size-Tiered Review Process

### Small Module (< 150 lines)

Target: 10 minutes.

1. **Architecture**: is the responsibility single?
2. **Error handling**: any silent error drops?
3. **Code smells**: high-severity only (> 80 line functions, todo!).

### Medium Module (150-500 lines)

Target: 20-30 minutes.

1. **Dimensions 1-4**: architecture & API surface, thread safety & state management, error handling & observability, documentation.
2. **Code smells**: high + medium severity.

### Large Module (> 500 lines)

Target: 45-60 minutes.

1. **All 8 dimensions** checked thoroughly.
2. **Code smells** full pass.
3. **Ancillary checklist** review.
4. **Data-flow trace**: entry point → exit point, end to end.

---

## Findings Report Format

```
P1 - Security vulnerability or data loss risk
P2 - Functional correctness bug or major performance defect
P3 - Design flaw or avoidable performance issue
P4 - Maintainability or code quality issue
P5 - Style or naming suggestion
```

Report by severity first, then by visibility (pub leak vs internal issue).

### Review Output Requirements

A real review must leave traceable evidence. At least one of:

| Output | Example |
|--------|---------|
| **Findings list** | "P2 - `Foo::bar()` returns `None` when `baz()` fails; caller may miss the error (line 42)" |
| **Confirmed-clear check record** | "Checked Mutex deadlock: found 2 `lock()` calls in `Foo`, neither in async context ✓" |
| **Question raised and answered** | "`MAX_RETRIES = 3` is hardcoded — should it be configurable? Decision: only one call site, not needed now" |

> Unacceptable output: `"Reviewed, no issues"` (7 words, means nothing was actually checked)
