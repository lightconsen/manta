  Tier 1 — High impact, tractable scope

  1. Self-repair loop

  OpenClaw's DefaultSelfRepair runs as a background task in the agent loop. It detects:
  - Stuck jobs — via ContextManager state age (context hasn't advanced in N turns)
  - Broken tools — via per-tool failure count in ToolRegistry

  Manta has the ToolRegistry already. Adding a failure_counts: RwLock<HashMap<String, u32>> field and a background task that checks context age would close this gap. Very high reliability value for production deployments.

  2. Trust-based skill attenuation

  OpenClaw's model: SkillTrust::Installed (read-only tools only) vs SkillTrust::Trusted (full tools). The lowest-trust active skill constrains the full tool set — so mixing a community skill with a trusted one doesn't escalate privileges.

  Manta's SkillMetadata already has a trusted: bool field but doesn't enforce attenuation. The fix is in SkillsManager::get_active_tools() — compute the minimum trust level across active skills and filter accordingly. Low code change, high security value.

  3. Skill gating enforcement

  OpenClaw checks GatingRequirements (required binaries, env vars, config paths, OS) before activating a skill, returning GatingError::MissingBinary etc. Manta has SkillRequires in its frontmatter struct but doesn't check it at activation time. Add a check_requirements() call in SkillsManager::activate() — this prevents runtime failures from missing dependencies.

  4. Live cost guard

  Manta has BudgetConfig and IterationBudget (per-agent iteration limits) but no live dollar/token spending tracker. OpenClaw's CostGuard tracks:
  - Daily spend in cents (with per-model token pricing)
  - Hourly action rate
  - Atomic budget_exceeded flag checked before every provider call

  Wiring this into Agent::get_completion() (alongside the existing needs_pruning check) would prevent runaway cost from loops or misbehaving tools.

  5. Pending approvals / human-in-the-loop gate

  OpenClaw's agent stores PendingApproval entries in the session before executing high-risk tools (shell, file write, etc.). The HTTP layer exposes /api/chat/approval to resolve them. Manta already has the ToolHooks infrastructure (BeforeHookFn) which is the right place to inject this. Adding a MANTA_REQUIRE_APPROVAL=shell,file_write config path and wiring it through hooks is a well-contained change that makes Manta safe for delegated use.

  ---
  Tier 2 — High impact, larger scope

  6. SSE + real-time event streaming

  This is the single biggest functional gap. Manta has process_message_with_progress that emits ProgressEvents — but they go nowhere because there's no SSE infrastructure in the gateway. OpenClaw's SseManager broadcasts typed events to web clients. Adding:
  - GET /api/events — SSE stream per client
  - Route ProgressEvent → SseEvent in the message processing worker

  This would unlock a real-time web UI and proper streaming responses without polling.

  7. OpenAI-compatible API (/v1/chat/completions)

  OpenClaw exposes this for drop-in ecosystem compatibility (Continue.dev, Open WebUI, etc.). Manta already understands CompletionRequest and CompletionResponse types internally. Mapping them to the OpenAI wire format is ~100 lines of JSON shaping. The payoff is compatibility with the entire LLM tooling ecosystem.

  8. Routine engine wired into the agent loop

  Manta now has AdvancedCronScheduler in the gateway — but it's not connected to the agent as a proactive trigger. OpenClaw's spawn_cron_ticker fires IncomingMessages into the agent from scheduled routines with quiet hours and timezone config. The pattern: on cron tick → create IncomingMessage with user_id="system" and a prompt → route through the normal process_message path. Low plumbing overhead, enables scheduled AI actions.

  9. Thread + Turn model with undo

  OpenClaw's Session → Thread → Vec<Turn> model is the cleanest architectural difference. Benefits:
  - Turn-level rollback (undo the last request without losing the conversation)
  - Parallel threads per session (multi-task conversations)
  - TurnState state machine (pending → running → complete/interrupted)

  Manta's current contexts: HashMap<String, Context> would need to grow a Thread level. This is a meaningful refactor but would unlock conversation branching and undo — features users expect from AI IDEs.

  ---
  Tier 3 — Medium impact, well-contained

  10. Deterministic skill prefilter

  Before activating skills OpenClaw runs keyword/regex matching (no LLM call) to determine which skills are relevant for the current message. Manta currently iterates all loaded skills and checks triggers. Adding prefilter_skills(message, available, max_skills, max_tokens) would reduce unnecessary skill activation and protect against prompt injection through skill system prompts.

  11. Per-tool failure count + circuit breaker

  Add failure_counts: RwLock<HashMap<String, u32>> to ToolRegistry. When a tool fails, increment. When count exceeds threshold (e.g., 3), mark tool as degraded and skip it from get_available(). Log a warning. This is 30 lines of code and prevents the agent from repeatedly calling a broken tool in a loop.

  12. Sliding window rate limiter in gateway

  Manta has RateLimiter in src/security/ but it's not confirmed to be wired into the HTTP gateway. OpenClaw gates: chat at 30 req/60s, OAuth at 10 req/60s. Add tower middleware on the Axum router — this is a one-liner with tower_http and prevents DoS on public deployments.

  13. Settings CRUD API (/api/settings)

  OpenClaw exposes list/get/put/delete/export/import for runtime settings. Manta has hot-reload for config files but no runtime settings mutation surface. Adding this would allow changing model, temperature, and tool config without restart and pairs well with the existing hot-reload infrastructure.

  ---
  Priority order summary

  | #   | Feature                                  | Effort | Value                 |
  |-----|------------------------------------------|--------|-----------------------|
  | 1   | Per-tool failure count + circuit breaker | XS     | Reliability           |
  | 2   | Skill gating enforcement at activation   | S      | Correctness           |
  | 3   | Trust-based skill attenuation            | S      | Security              |
  | 4   | Live cost guard in get_completion()      | S      | Cost safety           |
  | 5   | Self-repair background loop              | M      | Reliability           |
  | 6   | Pending approvals via ToolHooks          | M      | Safety for delegation |
  | 7   | Routine engine → agent loop wiring       | M      | Proactive AI          |
  | 8   | SSE event streaming in gateway           | M      | UX/streaming          |
  | 9   | OpenAI-compat /v1/chat/completions       | M      | Ecosystem             |
  | 10  | Deterministic skill prefilter            | M      | Performance           |
  | 11  | Sliding window rate limiter              | S      | Security              |
  | 12  | Thread + Turn + undo                     | L      | UX/correctness        |
  | 13  | Settings CRUD API                        | M      | Operability           |

  The first four (circuit breaker, skill gating, attenuation, cost guard) are all self-contained changes to existing structures and would close meaningful correctness/safety gaps with minimal risk. SSE and the OpenAI-compat API have the highest user-facing payoff among the larger items.