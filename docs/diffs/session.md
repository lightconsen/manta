# Manta Session vs OpenClaw Session — Detailed Comparison

> Last updated: 2026-05-15

## Overview

Both Manta and OpenClaw provide session management for multi-agent conversations. OpenClaw's session system is spread across multiple TypeScript files (`session.ts`, `group.ts`, `transcript.ts`, `artifacts.ts`, `resolve-route.ts`, `disk-budget.ts`), while Manta's is implemented in Rust modules under `src/agent/`.

**Current alignment: ~95%**

---

## Core Session Management

| Feature | OpenClaw | Manta | Status |
|---------|----------|-------|--------|
| **Session Identity** | UUID-based `session_id` | UUID-based `session_id` | Aligned |
| **Multi-Agent Sessions** | `Session` with `agents[]` | `MultiAgentSession` with `agents: HashMap` | Aligned |
| **Agent Lifecycle** | Spawn/terminate via messages | `SessionMessage::{SpawnAgent, TerminateAgent}` | Aligned |
| **Thread Binding** | `ThreadMode` (isolated, parent, shared) | `ThreadBinding` (Isolated, Parent, Shared, Existing) | Manta +1 variant |
| **Session Timeout** | Configurable TTL | `is_timed_out()` with `Duration` | Aligned |
| **Session Persistence** | File-based + in-memory | SQLite (`SessionStore`) + in-memory | Manta stronger |
| **Session Metadata** | Basic object | `SessionMetadata` with typed fields | Aligned |
| **Intent Routing** | Keyword-based routing | `find_agent_for_intent()` | Aligned |
| **Session Status** | `SessionStatus` object | `SessionStatus` struct | Aligned |

### Key Differences

- **Persistence**: OpenClaw uses file-based session storage. Manta uses SQLite with ACID guarantees, crash recovery, and structured querying via `SessionStore` (`session_store.rs`).
- **Thread Binding**: Manta adds `Existing(String)` binding mode, allowing agents to bind to arbitrary pre-existing threads. OpenClaw only supports `isolated`, `parent`, `shared`.
- **Context Model**: Manta uses a `Thread`/`Turn` model (`turns.rs`) with undo/redo support and compaction triggers. OpenClaw uses a flatter message history.

---

## Route Resolution

| Feature | OpenClaw (`resolve-route.ts`) | Manta (`route_resolution.rs`) | Status |
|---------|-------------------------------|-------------------------------|--------|
| **Multi-dimensional routing** | `RouteResolution` { peer, guild, team, account, channel, scope, roleBased } | `RouteResolution` with identical fields | Aligned |
| **Binding Cache** | In-memory with TTL | `BindingCache` with TTL + max_size + LRU eviction | Manta enhanced |
| **Route Rules** | `RouteRule` with glob patterns | `RouteRule` with `*` and prefix glob matching | Aligned |
| **Priority Ordering** | Higher priority wins | Sorted descending by priority | Aligned |
| **Session Overrides** | `session_overrides` map | `session_overrides` with cache invalidation | Aligned |
| **Binding Modes** | OneShot, Persistent, Ephemeral | `BindingMode::OneShot`, `Persistent`, `Ephemeral` | Aligned |
| **Conversation Scope** | DM, Channel, Thread | `ConversationScope::Dm`, `Channel`, `Thread` | Aligned |
| **Resolved Binding** | { threadId, agentId, mode, resolvedAt, explicit } | `ResolvedBinding` with identical fields | Aligned |

### Key Differences

- **Cache Eviction**: Manta's `BindingCache` implements a proper LRU eviction strategy when capacity is exceeded. OpenClaw uses a simple unbounded map.
- **Cache Invalidation**: Manta's `clear_session_override()` also clears the cache to prevent stale bindings. OpenClaw does not explicitly invalidate on override removal.

---

## Transcript System

| Feature | OpenClaw (`transcript.ts`) | Manta (`transcript.rs`) | Status |
|---------|----------------------------|-------------------------|--------|
| **Message Model** | `TranscriptMessage` { role, content, timestamp, metadata } | `TranscriptMessage` identical structure | Aligned |
| **Transcript Model** | `Transcript` { sessionId, channel, peer, scope, messages[] } | `Transcript` with identical fields | Aligned |
| **Export Formats** | JSON, Markdown, Text, HTML | `TranscriptFormat::Json`, `Markdown`, `Text`, `Html` | Aligned |
| **Format Extension** | `.json`, `.md`, `.txt`, `.html` | Same extensions via `extension()` | Aligned |
| **MIME Types** | Included | `mime_type()` method | Aligned |
| **File Storage** | Root directory per format | `TranscriptStore` with `root_dir` | Aligned |
| **Active Buffer** | In-memory accumulation | `active: Mutex<HashMap>` for live sessions | Aligned |
| **Render Functions** | `renderMarkdown()`, `renderText()`, etc. | `render_transcript()` + format-specific fns | Aligned |
| **HTML Rendering** | Basic HTML page | Full HTML with CSS styling + `html_escape` | Manta enhanced |
| **Load from File** | `load()` | `load()` from JSON file | Aligned |
| **Stats** | Basic counts | `TranscriptStoreStats` { active_sessions, total_messages, total_file_count } | Aligned |

### Key Differences

- **HTML Rendering**: Manta produces a styled HTML page with CSS classes per role (`user`, `assistant`, `system`, `tool`). OpenClaw's HTML output is more basic.
- **Timestamps**: OpenClaw uses ISO strings. Manta uses `chrono::DateTime<Utc>` with formatted output.

---

## Artifacts System

| Feature | OpenClaw (`artifacts.ts`) | Manta (`artifacts.rs`) | Status |
|---------|---------------------------|------------------------|--------|
| **Artifact Types** | Code, Document, Image, Link, Data, File | `ArtifactType::Code`, `Document`, `Image`, `Link`, `Data`, `File` | Aligned |
| **Artifact Model** | `Artifact` with id, sessionId, title, type, content, filePath, language, url, size | `Artifact` with identical fields + `mime_type`, `tags`, `metadata` | Manta enhanced |
| **Factory Methods** | `code()`, `document()`, `link()`, `data()` | Same factory methods + builder-style `with_tag()`, `with_metadata()` | Aligned |
| **Content Retrieval** | `getContent()` from memory or file | `get_content()` same behavior | Aligned |
| **Markdown Rendering** | `toMarkdown()` per type | `to_markdown()` matching all types | Aligned |
| **Session-bound Lifecycle** | Auto-cleanup on session end | `clear_session()` removes all for session | Aligned |
| **File-backed Storage** | Large artifacts on disk | `file_path` field + `root_dir` | Aligned |
| **Store API** | `add()`, `get()`, `list()`, `remove()`, `clearSession()` | `add()`, `get()`, `get_for_session()`, `list_all()`, `list_session()`, `remove()`, `clear_session()` | Manta enhanced |
| **Export** | Session export to markdown file | `export_session()` to `.md` | Aligned |
| **Stats** | Basic counts | `ArtifactStoreStats` { session_count, artifact_count, total_size_bytes } | Aligned |

### Key Differences

- **Extra Fields**: Manta's `Artifact` includes `mime_type`, `tags`, and `metadata` for richer artifact descriptions.
- **Builder Pattern**: Manta adds `with_tag()`, `with_metadata()`, `with_file_path()` for fluent construction.
- **Store Methods**: Manta adds `list_session()` (list IDs for a session) and `list_all()` (all artifacts across sessions).

---

## Disk Budget / Session Quota

| Feature | OpenClaw (`disk-budget.ts`) | Manta (`disk_budget.rs`) | Status |
|---------|-----------------------------|--------------------------|--------|
| **Per-Session Budget** | `SessionBudget` with limit_bytes | `SessionBudget` with `limit_bytes` + `used_bytes` | Aligned |
| **Default Budget** | 10 MB per session | `DEFAULT_SESSION_BUDGET_BYTES = 10 MB` | Aligned |
| **Eviction Strategies** | LRU, OldestFirst, LargestFirst, Reject | `EvictionStrategy::Lru`, `OldestFirst`, `LargestFirst`, `Reject` | Aligned |
| **Item Tracking** | Items with size, createdAt, lastAccessed | `BudgetItem` with identical fields | Aligned |
| **Category Tracking** | Artifact, Transcript, File, Cache | `BudgetCategory` enum with same variants | Aligned |
| **Global Manager** | `DiskBudgetManager` for all sessions | `DiskBudgetManager` with `HashMap<String, SessionBudget>` | Aligned |
| **User Index** | N/A | `user_index` for reverse lookup | Manta extra |
| **Stats** | Per-session and global | `SessionBudgetStats` + `GlobalBudgetStats` | Aligned |
| **Over-budget Check** | `isOverBudget()` | `is_over_budget()` | Aligned |

### Key Differences

- **User Index**: Manta's `DiskBudgetManager` maintains a `user_index` mapping users to their groups (inherited from group session design).
- **Error Types**: Manta uses `thiserror`-derived `DiskBudgetError` with typed variants (`ItemTooLarge`, `BudgetExceeded`). OpenClaw uses plain Error objects.

---

## Group Sessions

| Feature | OpenClaw (`group.ts`) | Manta (`group.rs`) | Status |
|---------|-----------------------|--------------------|--------|
| **Member Roles** | Owner, Admin, Member, Observer | `GroupRole::Owner`, `Admin`, `Member`, `Observer` | Aligned |
| **Role Permissions** | `canManageMembers`, `canTerminateSession`, etc. | Same permission methods on `GroupRole` | Aligned |
| **Group Session Model** | `GroupSession` with members, ownerId | `GroupSession` with `members: HashMap` | Aligned |
| **Member Management** | Add, remove, update role | Same operations + `mark_active`/`mark_inactive` | Manta enhanced |
| **Owner Protection** | Cannot remove owner | `CannotRemoveOwner` error | Aligned |
| **Role Level Checks** | `hasRole()` with min role | `has_role()` using `role_level()` hierarchy | Aligned |
| **Group Manager** | `GroupSessionManager` | `GroupSessionManager` with user index | Manta enhanced |
| **User-to-Groups Index** | Present | `user_index: HashMap<String, Vec<String>>` | Aligned |
| **Stats** | Basic counts | `GroupManagerStats` { group_count, total_members } | Aligned |
| **Archive** | `archive()` method | `archive()` + `is_archived` flag | Aligned |

### Key Differences

- **Async API**: Manta's `GroupSessionManager` uses async methods (`add_member`, `remove_member`, `remove_group`) with `RwLock` for concurrent access. OpenClaw uses synchronous operations.
- **Member Activity**: Manta tracks `is_active` per member with `mark_active()`/`mark_inactive()`.

---

## Manta-Exclusive Session Features (Not in OpenClaw)

| Feature | Module | Description |
|---------|--------|-------------|
| **SQLite Persistence** | `session_store.rs` | ACID session storage with `sqlx` SQLite pool |
| **Turn-based Thread Model** | `turns.rs` | `Thread` with `Turn` log, undo/redo, compaction triggers |
| **Context Compaction** | `compaction.rs` | LLM-assisted context summarization when over budget |
| **Cost Guard** | `cost_guard.rs` | Per-session daily/hourly spend limits |
| **Response Cache** | `mod.rs` | LLM-determined cacheability with TTL |
| **Task Planning** | `planner.rs` | Automatic task decomposition with `ActivePlan` |
| **Subagent Registry** | `subagent_registry.rs` | Metrics and status tracking for spawned subagents |
| **Personality Memory** | `personality.rs` | Agent personality persistence with `AgentRegistry` |
| **Session Search** | `memory::SessionSearch` | FTS5 full-text indexing of conversation history |
| **Vector Memory** | `memory::vector` | Semantic search over session content |
| **Hybrid Search** | `memory::MemoryManager` | Vector + FTS5 + MMR re-ranking |

---

## OpenClaw-Exclusive Session Features (Not in Manta)

| Feature | Module | Gap |
|---------|--------|-----|
| **Voice / TTS Integration** | N/A | Large gap — requires TTS engine |
| **Real-time Collaboration** | N/A | OpenClaw has live cursor/typing indicators for group sessions |
| **Session Replay** | N/A | OpenClaw can replay a session turn-by-turn in UI |

---

## File Mapping

| OpenClaw File | Manta File | Lines |
|---------------|------------|-------|
| `session.ts` | `src/agent/session.rs` | ~584 |
| `group.ts` | `src/agent/group.rs` | ~508 |
| `transcript.ts` | `src/agent/transcript.rs` | ~648 |
| `artifacts.ts` | `src/agent/artifacts.rs` | ~487 |
| `resolve-route.ts` | `src/agent/route_resolution.rs` | ~645 |
| `disk-budget.ts` | `src/agent/disk_budget.rs` | ~405 |
| `context.ts` | `src/agent/context.rs` | ~300+ |
| N/A | `src/agent/session_store.rs` | ~400+ |
| N/A | `src/agent/turns.rs` | ~200+ |
| N/A | `src/agent/compaction.rs` | ~200+ |

**Total**: OpenClaw ~2,500 lines (TypeScript) vs Manta ~4,300 lines (Rust) across session-related files.

---

## Summary

Manta's session system is now **functionally equivalent** to OpenClaw's with several enhancements:

1. **Stronger Persistence**: SQLite ACID vs OpenClaw's file-based approach
2. **Better Caching**: LRU eviction in `BindingCache`
3. **Richer Artifacts**: Tags, metadata, MIME types
4. **Turn Model**: Undo/redo support with compaction triggers
5. **Cost Controls**: Built-in spend guarding
6. **Search**: FTS5 + vector semantic search over sessions

The remaining ~5% gap is primarily in **voice/TTS integration** and **real-time collaboration UI features**, which are outside core session mechanics.
