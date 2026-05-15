# Manta Gateway vs OpenClaw Gateway — Current Comparison

> Last updated: 2026-05-15

## Executive Summary

Manta Gateway has closed most functional gaps with OpenClaw Gateway. What was previously marked as "not implemented" in the original diff document is now largely complete. The remaining gaps are primarily in channel coverage (Signal/iMessage), voice capabilities, and the plugin ecosystem.

**Current alignment: ~88%**

---

## Filled Gaps (Previously Marked Missing)

| Feature | Status | Implementation |
|---------|--------|----------------|
| **DM Pairing** | Complete | `src/security/pairing.rs` — reactive pairing with approve/reject/revoke |
| **Mention Gating** | Complete | `src/gateway/mod.rs` + `ChannelConfig.require_mention` |
| **Command Gating** | Complete | `src/tools/command_gate.rs` + `/api/v1/gate/*` endpoints |
| **Runtime Audit Logging** | Complete | `src/security/runtime_audit.rs` + `GET /api/v1/audit/log` |
| **Web Control UI** | Complete | `assets/control_ui/` — 11-page dashboard |
| **Subagent Spawning** | Complete | `src/acp/mod.rs` + `/api/v1/acp/sessions` |
| **Hot Reload** | Complete | `src/config/hot_reload.rs` — type-safe handlers |
| **Event Hooks** | Complete | `src/gateway/hooks.rs` — `EventHookRegistry` |
| **OAuth Authentication** | Complete | `src/gateway/auth/oauth.rs` — GitHub + Google |
| **CSP / CORS Headers** | Complete | `src/security/mod.rs` — `SecurityHeaders` with CSP nonce |
| **Rate Limiting** | Complete | `src/security/sliding_window.rs` + `src/gateway/rate_limit.rs` |
| **Config Runtime API** | Complete | `PUT /api/v1/config` + `POST /api/v1/config/validate` |
| **WebSocket Protocol** | Enhanced | `src/gateway/ws.rs` — subscribe/unsubscribe + token auth |
| **Sliding Window Rate Limiter** | Complete | `src/security/sliding_window.rs` |
| **Approval Queue** | Complete | `src/tools/approval.rs` — human-in-the-loop |
| **Route Resolution** | Complete | `src/agent/route_resolution.rs` — multi-dimensional routing |
| **Transcript System** | Complete | `src/agent/transcript.rs` — JSON/Markdown/HTML/Text export |
| **Artifacts** | Complete | `src/agent/artifacts.rs` — session-bound snippets, docs, links |
| **Disk Budget** | Complete | `src/agent/disk_budget.rs` — per-session quota + LRU eviction |
| **Group Sessions** | Complete | `src/agent/group.rs` — member roles + group scoping |

---

## Remaining Gaps

### Medium Gaps

| Feature | OpenClaw | Manta | Notes |
|---------|----------|-------|-------|
| **Slack Integration** | Full Bolt SDK with events | Webhook stub only | Needs Slack Events API + socket mode |
| **WebChat Interface** | Independent web chat app | Web Terminal (embedded in admin) | Could be extracted to standalone page |
| **Signal Channel** | signal-cli integration | Not implemented | Requires signal-cli or libsignal FFI |
| **iMessage Channel** | BlueBubbles integration | Not implemented | macOS only, BlueBubbles bridge |
| **Channel Count** | 20+ | 6 | Core messaging channels covered; niche channels missing |

### Large Gaps

| Feature | OpenClaw | Manta | Notes |
|---------|----------|-------|-------|
| **Voice / TTS** | Text-to-speech + voice wake | Not implemented | Requires TTS engine integration (e.g. piper, coqui) |
| **Plugin SDK** | jiti runtime ESM hot-loading | WASM foundation only | Would need Lua/Rhino/QuickJS runtime |
| **Mobile Apps** | iOS + Android companions | None | Separate project scope |
| **Auth Profile Rotation** | Multi-key rotation with cooldown | Single API key per provider | Provider-level enhancement |

### Small Gaps

None remaining. All previously identified small gaps (Session Files, Artifacts, Disk Budget) are now implemented.

---

## Manta Advantages Over OpenClaw

| Feature | Manta | OpenClaw |
|---------|-------|----------|
| **Circuit Breaker** | Full state machine (Closed/Open/HalfOpen) | Not implemented |
| **Tailscale Integration** | Built-in remote access | Not available |
| **Cron Scheduler** | Production-grade: At/Every/Cron + retry + crash recovery | Basic cron only |
| **Hybrid Search** | Vector + FTS5 + MMR re-ranking | Partial |
| **Runtime Provider API** | Hot-switch via REST | CLI commands only |
| **Cost Guard** | Daily limit + hourly action rate limiting | Basic usage tracking |
| **Single Binary** | `cargo build` produces one binary | Node.js + pnpm dependencies |
| **Rust Performance** | Low memory, no GC pauses | V8 heap, larger footprint |

---

## Feature Matrix

| Feature | Manta | OpenClaw | Gap |
|---------|:-----:|:--------:|:---:|
| **Core Gateway** | | | |
| HTTP Framework (Axum/Express) | | | None |
| WebSocket API | | | None |
| REST API | | | None |
| OAuth (GitHub/Google) | | | None |
| API Key Auth | | | None |
| Rate Limiting | | | None |
| CSP Security Headers | | | None |
| **Control Plane** | | | |
| Web Control UI | | | None |
| Hot Config Reload | | | None |
| Event Hooks | | | None |
| Audit Logging | | | None |
| **Channels** | | | |
| Telegram | | | None |
| Discord | | | None |
| WhatsApp | | | None |
| Feishu/Lark | | | None |
| Slack | Stub | Full | Medium |
| Signal | | | Large |
| iMessage | | | Large |
| WebChat | Terminal | Full app | Medium |
| **Access Control** | | | |
| DM Pairing | | | None |
| Mention Gating | | | None |
| Command Gating | | | None |
| Allowlist (pattern matching) | | | None |
| Blocklist | | | None |
| **Agent System** | | | |
| Agent Spawning | | | None |
| Subagent Spawning (ACP) | | | None |
| Session Management | | | None |
| Multi-Agent Routing | | | None |
| Route Resolution | | | None |
| Group Sessions | | | None |
| **Memory** | | | |
| SQLite Chat History | | | None |
| Vector Search | | | None |
| Hybrid Search | | | Manta leads |
| Embeddings (local + API) | | | None |
| Session Files (Transcripts) | | | None |
| Artifacts | | | None |
| Disk Budget | | | None |
| **Outbound Pipeline** | | | |
| Trajectory Logging | | | None |
| Canvas / A2UI | | | None |
| SSE Streaming | | | None |
| Reply Dispatcher | | | None |
| Side Effects | | | None |
| **Tools** | | | |
| File Tools | | | None |
| Shell Execution | | | None |
| Browser Automation | | | None |
| Web Search | | | None |
| Canvas Tools | | | None |
| Tool Hooks | | | None |
| Approval Queue | | | None |
| Subagent Tools | | | None |
| Plugin Tools | | | Large |
| **Infrastructure** | | | |
| Circuit Breaker | | | Manta leads |
| Tailscale | | | Manta leads |
| Cron Jobs | | | Manta leads |
| Voice / TTS | | | Large |
| Plugin SDK | WASM only | Full jiti | Large |
| Mobile Apps | | | Large |

---

## Next Steps (Priority Order)

1. **Slack Full Implementation** — Replace webhook stub with Slack Events API + socket mode for real-time messaging
2. **Signal Channel** — Integrate signal-cli or libsignal for Signal messaging support
3. **Standalone WebChat** — Extract Web Terminal into an independent `/chat` page outside the admin UI
4. **iMessage Channel** — BlueBubbles bridge integration (macOS only)
5. **Plugin SDK** — Add Lua or QuickJS runtime for non-WASM dynamic plugin loading
6. **Voice / TTS** — Integrate a lightweight TTS engine (e.g. piper) for voice responses

---

## Code Size (Gateway Module Only)

| Metric | Manta | OpenClaw |
|--------|-------|----------|
| **Lines** | ~8,500 (`src/gateway/mod.rs`) | ~15,000 |
| **Files** | 12 | 245+ |
| **Languages** | Rust | TypeScript |

Manta's Gateway is more compact due to Rust's expressiveness and Axum's minimal boilerplate, while achieving comparable feature coverage.
