# Manta Memory vs OpenClaw Memory/Persistence — Detailed Comparison

> Last updated: 2026-05-15

## Overview

Both Manta and OpenClaw provide a multi-layered memory and persistence system for conversational AI. OpenClaw's memory system is built in TypeScript around multiple backends (builtin, QMD, LanceDB), while Manta's is implemented in Rust under `src/memory/` with SQLite as the single unified backend, supporting vector search, FTS5 full-text search, hybrid search, and local GGUF embeddings.

**Current alignment: ~85%**

---

## Core Memory Architecture

| Feature | OpenClaw | Manta (`src/memory/mod.rs`) | Status |
|---------|----------|----------------------------|--------|
| **Backend** | Multiple (builtin, qmd, lancedb) | SQLite (single unified) | Manta simplified |
| **Trait/Interface** | Backend-specific | `MemoryStore` + `ChatHistoryStore` traits | Aligned concept |
| **Core Entry** | `MemoryEntry` | `Memory` struct with embedding | Aligned |
| **Message Entry** | `ChatMessage` | `ChatMessage` | Aligned |
| **Unique ID** | UUID | `MemoryId(pub String)` — UUID | Aligned |
| **Metadata** | `metadata: Record<string, unknown>` | `metadata: serde_json::Value` | Aligned |
| **Importance Score** | ✅ | `importance_score: f32` | Aligned |
| **Expiration** | ✅ TTL | `expires_at: Option<DateTime<Utc>>` | Aligned |
| **Embedding Storage** | Vector DB native | SQLite BLOB (f32 little-endian) | Manta different |
| **Cosine Similarity** | DB-native | `cosine_similarity()` pure Rust | Aligned |

### Key Differences

- **Unified Backend**: Manta uses SQLite for everything (chat history, semantic memory, vector storage, FTS5), while OpenClaw supports multiple backends (builtin file-based, QMD, LanceDB).
- **Embedding in SQLite**: Manta serializes embeddings as little-endian f32 BLOBs in SQLite, enabling vector search without an external vector DB. OpenClaw relies on the vector DB backend for embedding storage.

---

## Database Layer (SQLite)

| Feature | OpenClaw | Manta (`src/memory/db.rs`) | Status |
|---------|----------|---------------------------|--------|
| **Database** | Multiple (SQLite optional) | SQLite (sqlx) — primary and only | Manta simplified |
| **WAL Mode** | Depends on backend | ✅ WAL + `synchronous=NORMAL` | Manta extra |
| **Schema Init** | Backend-dependent | `init_schema()` — idempotent | Aligned |
| **Migration** | Manual / backend-specific | `migrate_schema()` — idempotent column adds | Manta extra |
| **FTS5** | Depends on backend | ✅ FTS5 virtual table + sync triggers | Manta extra |
| **Access Tracking** | ❌ | `last_accessed_at` column + auto-update | Manta extra |
| **Optimization** | ❌ | `optimize()` — pragmas + `ANALYZE` | Manta extra |
| **Fragmentation Stats** | ❌ | `DbStats::fragmentation_percent()` | Manta extra |
| **In-Memory Mode** | ❌ | `new_in_memory()` for testing | Manta extra |
| **Batch Insert** | ✅ | `with_batch_size(100)` | Aligned |

### Schema (Manta)

```sql
-- Core tables
conversations    (id, user_id, channel, title, created_at, updated_at)
messages         (id, conversation_id, user_id, role, content, created_at, metadata)
memories         (id, user_id, conversation_id, content, memory_type, embedding,
                  created_at, expires_at, metadata, importance_score, source, last_accessed_at)

-- FTS5 for full-text search
messages_fts     (content)  -- virtual table with triggers

-- Indexes
idx_memories_user, idx_memories_conversation, idx_memories_type
idx_messages_conversation, idx_memories_expires_at
```

### Key Differences

- **WAL + Pragmas**: Manta's `DatabaseStore::optimize()` applies SQLite performance pragmas (WAL, foreign keys, cache_size, mmap_size) automatically.
- **FTS5 Integration**: Manta creates an FTS5 virtual table with INSERT/DELETE/UPDATE triggers to keep it in sync with the `messages` table — enabling full-text search without external dependencies.
- **Access Tracking**: The `last_accessed_at` column is updated on every `get()` call, enabling LRU-like eviction policies.

---

## Vector Search & Embeddings

| Feature | OpenClaw | Manta (`src/memory/vector.rs`) | Status |
|---------|----------|-------------------------------|--------|
| **Vector DB** | QMD / LanceDB | SQLite BLOB + in-memory `MemoryVectorStore` | Manta different |
| **Embedding Providers** | Gemini, OpenAI, Voyage | `ApiEmbeddingProvider` + `LocalGgufEmbeddingProvider` | Aligned |
| **Local Embeddings** | ❌ | ✅ `llama-cpp-2` GGUF models | Manta extra |
| **Dimension Config** | Backend-specific | `EmbeddingConfig::dimension` | Aligned |
| **Chunking** | `embedding-chunk-limits.ts` | `TextChunker` (word-based, overlapping) | Aligned |
| **Batch Embedding** | Provider-specific batching | `BatchEmbeddingProcessor` | Aligned |
| **Embedding Cache** | ❌ | `CachedEmbeddingProvider` (SHA-256 dedup, FIFO) | Manta extra |
| **Collection Support** | ✅ Collections | `VectorMemoryService::search_collection()` | Aligned |
| **Pipeline** | ❌ | `EmbeddingPipeline` (background batch worker) | Manta extra |

### Embedding Providers (Manta)

```rust
pub trait EmbeddingProvider: Send + Sync {
    async fn model_name(&self) -> String;
    async fn dimension(&self) -> usize;
    async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String>;
}
```

**Implementations:**
- `ApiEmbeddingProvider` — OpenAI-style API (reqwest-based)
- `LocalGgufEmbeddingProvider` — Local GGUF via `llama-cpp-2` (feature-gated)
- `CachedEmbeddingProvider<P>` — SHA-256 dedup cache wrapper around any provider

### Key Differences

- **Local GGUF**: Manta supports running embedding models locally via `llama-cpp-2`, with automatic HuggingFace Hub download (`hf:` prefix support). OpenClaw requires an API provider.
- **Embedding Cache**: Manta's `CachedEmbeddingProvider` deduplicates identical texts via SHA-256 hash with FIFO eviction, reducing API costs.
- **Embedding Pipeline**: Manta's `EmbeddingPipeline` runs as a background Tokio task, batching requests (max 32, 100ms wait window) for throughput optimization.

---

## Hybrid Search

| Feature | OpenClaw | Manta (`src/memory/hybrid.rs`) | Status |
|---------|----------|-------------------------------|--------|
| **Vector + Text** | Partial | ✅ `hybrid_search()` with concurrent execution | Manta extra |
| **Score Normalization** | ❌ | ✅ Independent normalization to [0, 1] | Manta extra |
| **Weighted Fusion** | ❌ | ✅ Configurable `vector_weight` + `text_weight` | Manta extra |
| **Deduplication** | ❌ | ✅ SHA-256 content fingerprint dedup | Manta extra |
| **Temporal Decay** | ❌ | ✅ Exponential decay by citation date | Manta extra |
| **MMR Re-ranking** | ❌ | ✅ Maximal Marginal Re-ranking for diversity | Manta extra |
| **Jaccard Similarity** | ❌ | ✅ Word-level Jaccard for MMR | Manta extra |

### Hybrid Search Pipeline (Manta)

```
1. Run vector search + FTS5 search concurrently (tokio::join!)
2. Normalize scores independently to [0, 1]
3. Merge with weighted average: vector_weight * v_score + text_weight * fts_score
4. Deduplicate by SHA-256 content fingerprint
5. Apply temporal decay (optional): score * exp(-ln(2) * age_days / half_life)
6. MMR re-ranking (optional): balance relevance vs diversity
7. Return top-N results
```

### Key Differences

- **Concurrent Execution**: Manta runs vector and FTS5 searches concurrently via `tokio::join!`.
- **MMR Re-ranking**: Manta implements Maximal Marginal Re-ranking to balance relevance with result diversity — OpenClaw has no equivalent.
- **Temporal Decay**: Manta can apply exponential temporal decay based on dates extracted from citation text.

---

## Chat History / Session Search

| Feature | OpenClaw | Manta (`src/memory/session_search.rs`) | Status |
|---------|----------|--------------------------------------|--------|
| **Storage** | `transcripts.ts` (file-based) | SQLite `messages` table | Manta different |
| **FTS5 Search** | ❌ | ✅ `SessionSearch::search()` | Manta extra |
| **Context Retrieval** | ❌ | ✅ `get_context()` — surrounding messages | Manta extra |
| **Relevance Scoring** | ❌ | ✅ BM25-like ranking via FTS5 | Manta extra |
| **Filter by User** | ✅ | ✅ `for_user()` builder | Aligned |
| **Filter by Conversation** | ✅ | ✅ `for_conversation()` builder | Aligned |
| **Stats** | Basic | `SessionStats` — counts + indexed | Manta enhanced |
| **Cleanup** | ❌ | `cleanup_before(date)` | Manta extra |

### SessionSearch Query Builder

```rust
let results = session_search
    .search(
        SessionSearchQuery::new("deployment issue")
            .for_user("user-123")
            .limit(10)
            .with_context(2), // 2 messages before/after
    )
    .await?;
```

### Key Differences

- **FTS5-Powered**: Manta's session search uses SQLite FTS5 with automatic triggers, providing relevance-ranked full-text search over all conversation history.
- **Context Retrieval**: Manta fetches surrounding messages (configurable context lines) for each search result, providing conversational context.

---

## Memory Manager / Orchestration

| Feature | OpenClaw | Manta (`src/memory/manager.rs`) | Status |
|---------|----------|--------------------------------|--------|
| **Unified Orchestrator** | ❌ (backend-specific) | ✅ `MemoryManager` | Manta extra |
| **Observe API** | Backend-specific | `observe(user_id, content, type, importance)` | Aligned |
| **Retrieve API** | Backend-specific | `retrieve(user_id, conversation, query, limit)` | Aligned |
| **Session Context** | ❌ | `session_context()` — episodic + semantic | Manta extra |
| **Context Formatting** | ❌ | `SessionContext::format_for_injection()` | Manta extra |
| **Context Cache** | ❌ | `ContextCache` — 5-second TTL | Manta extra |
| **Message Remembering** | ✅ | `remember_message()` | Aligned |
| **Session Compaction** | ❌ | `compact_session()` — extract facts to memories | Manta extra |
| **Builder Pattern** | ❌ | `MemoryManagerBuilder` | Manta extra |

### MemoryManager Architecture

```rust
pub struct MemoryManager {
    store: Arc<UnifiedStore>,
    config: MemoryManagerConfig,
    pipeline: Option<EmbeddingPipelineHandle>,
    vector_service: Option<Arc<VectorMemoryService>>,
    session_search: Option<Arc<SessionSearch>>,
    context_cache: ContextCache,
}
```

**Key methods:**
- `observe()` — Primary write path for semantic memories (stores + embeds)
- `retrieve()` — Hybrid search path (vector + FTS5) or fallback to DB search
- `session_context()` — Returns both episodic (recent messages) and semantic (relevant memories) context
- `compact_session()` — Extracts key facts from old messages into semantic memories

### Key Differences

- **Unified Orchestrator**: Manta's `MemoryManager` is a single entry point that coordinates the database, vector service, session search, and embedding pipeline. OpenClaw lacks a unified orchestrator.
- **Session Context**: Manta's `session_context()` returns both episodic (recent chat history) and semantic (relevant memories) context in a single call, formatted for LLM injection.
- **Context Cache**: Manta caches recent context retrievals with a 5-second TTL to avoid redundant queries during rapid turn-taking.

---

## Personality Memory System

| Feature | OpenClaw | Manta (`src/memory/personality.rs`) | Status |
|---------|----------|------------------------------------|--------|
| **Memory Files** | SOUL.md, IDENTITY.md, BOOTSTRAP.md | Same + AGENTS.md, TOOLS.md, HEARTBEAT.md, MEMORY.md, USER.md | Manta enhanced |
| **File Types** | 3 | 8 (`MemoryType` enum) | Manta enhanced |
| **CRUD Operations** | Basic file I/O | `read/write/append/clear/exists/size` | Aligned |
| **Prompt Formatting** | Static concatenation | `format_for_prompt()` with budget enforcement | Manta enhanced |
| **Context Variants** | ❌ | `MemoryContext::Primary` vs `Subagent` | Manta extra |
| **File Caching** | ❌ | `read_with_cache()` — mtime/size based | Manta extra |
| **Security Scan** | ❌ | `scan_for_threats()` — injection pattern detection | Manta extra |
| **Truncation** | ❌ | `truncate_with_head_tail()` — 70/20 split | Manta extra |
| **Budget Enforcement** | ❌ | `DEFAULT_MAX_MEMORY_SIZE` (20K) + `DEFAULT_TOTAL_MAX_SIZE` (150K) | Manta extra |
| **Memory Fragments** | ❌ | `load_memory_fragments()` — dated `memory/*.md` files | Manta extra |
| **Tool Integration** | ❌ | `PersonalityMemoryTool` (read/write/append/clear) | Manta extra |

### MemoryType Enum (Manta)

```rust
pub enum MemoryType {
    Soul,       // SOUL.md — core identity
    Identity,   // IDENTITY.md — display name, persona
    Bootstrap,  // BOOTSTRAP.md — startup instructions
    User,       // USER.md — user preferences
    Agents,     // AGENTS.md — agent registry
    Tools,      // TOOLS.md — tool definitions
    Heartbeat,  // HEARTBEAT.md — periodic tasks
    Memory,     // MEMORY.md — curated long-term memory
}
```

### Key Differences

- **Extra Files**: Manta adds 5 additional personality files beyond OpenClaw's 3 (AGENTS, TOOLS, HEARTBEAT, MEMORY, USER).
- **Budget Enforcement**: Manta enforces per-file (20K chars) and total (150K chars) size limits with smart truncation (keeps first 70% + last 20%).
- **Security Scan**: Manta scans personality files for injection patterns (`<script`, `javascript:`, `eval(`, etc.) before loading.
- **Memory Fragments**: Manta loads dated `memory/*.md` fragments chronologically for temporal memory accumulation.

---

## Memory Flush / Compaction

| Feature | OpenClaw | Manta (`src/memory/flush.rs`) | Status |
|---------|----------|------------------------------|--------|
| **Flush Decision** | Integrated in ACP | `check_memory_flush()` — heuristic-based | Aligned |
| **Token Threshold** | ✅ | ✅ `soft_threshold_tokens` | Aligned |
| **Transcript Size** | ✅ | ✅ `force_flush_transcript_bytes` | Aligned |
| **Deduplication** | ❌ | SHA-256 context hash + compaction count | Manta enhanced |
| **Flush Target** | File-based | `memory/YYYY-MM-DD.md` | Aligned |
| **Flush Reason Tracking** | ❌ | `FlushReason` enum | Manta extra |

### Flush Decision Logic (Manta)

```rust
pub fn check_memory_flush(
    total_tokens: usize,
    transcript_bytes: usize,
    config: &ContextCompressorConfig,
    context_window: usize,
    compaction_state: &SessionCompactionState,
    current_messages: &[Message],
) -> MemoryFlushDecision {
    // 1. Deduplication: skip if same context hash
    // 2. Deduplication: skip if compaction count unchanged
    // 3. Token threshold: total_tokens > context_window - reserve - soft_threshold
    // 4. Transcript size: transcript_bytes > force_flush_transcript_bytes
}
```

### Key Differences

- **Deduplication**: Manta tracks compaction count and SHA-256 context hash to prevent redundant flushes within the same compaction cycle.
- **Flush Reason**: Manta categorizes flush triggers (`TokenThreshold`, `TranscriptSize`, `None`) for observability.

---

## Workspace State

| Feature | OpenClaw | Manta (`src/memory/workspace_state.rs`) | Status |
|---------|----------|----------------------------------------|--------|
| **Workspace Tracking** | ❌ | `WorkspaceState` + `WorkspaceManager` | Manta extra |
| **Bootstrap Seeding** | ❌ | `is_bootstrap_seeded()` / `mark_bootstrap_seeded()` | Manta extra |
| **Setup Completion** | ❌ | `is_setup_completed()` / `mark_setup_completed()` | Manta extra |
| **Git Init** | ❌ | `ensure_git_repo()` | Manta extra |
| **State Persistence** | ❌ | Atomic JSON write (temp + rename) | Manta extra |
| **Legacy Migration** | ❌ | `load()` with version migration | Manta extra |

### WorkspaceState

```rust
pub struct WorkspaceState {
    version: u32,                          // Currently 1
    bootstrap_seeded_at: Option<String>,   // ISO 8601 timestamp
    setup_completed_at: Option<String>,   // ISO 8601 timestamp
}
```

### Key Differences

- **Workspace Lifecycle**: Manta tracks workspace initialization state (bootstrap seeded, setup completed) to guide first-time user experience.
- **Git Integration**: Manta automatically initializes a git repository for new workspaces.

---

## Manta-Exclusive Memory Features (Not in OpenClaw)

| Feature | Module | Description |
|---------|--------|-------------|
| **Unified MemoryManager** | `manager.rs` | Single orchestrator for DB, vector, search, pipeline |
| **Hybrid Search** | `hybrid.rs` | Vector + FTS5 with MMR re-ranking and temporal decay |
| **Session Context** | `manager.rs` | Episodic + semantic context in one call |
| **Context Cache** | `manager.rs` | 5-second TTL cache for context retrievals |
| **Local GGUF Embeddings** | `local_embeddings.rs` | On-device embedding via llama-cpp-2 |
| **Embedding Pipeline** | `pipeline.rs` | Background batched embedding worker |
| **Embedding Cache** | `vector.rs` | SHA-256 dedup cache with FIFO eviction |
| **FTS5 Session Search** | `session_search.rs` | Full-text search over conversation history |
| **Search Context** | `session_search.rs` | Surrounding message retrieval |
| **PersonalityMemoryTool** | `personality.rs` | Tool trait impl for memory CRUD |
| **Security Scan** | `personality.rs` | Injection pattern detection |
| **Memory Fragments** | `personality.rs` | Dated `memory/*.md` chronological loading |
| **Budget Enforcement** | `personality.rs` | 20K per-file / 150K total limits |
| **Context Variants** | `personality.rs` | Primary vs Subagent prompt variants |
| **File Cache** | `personality.rs` | mtime/size-based caching |
| **Memory Flush Deduplication** | `flush.rs` | SHA-256 hash + compaction count tracking |
| **Workspace State** | `workspace_state.rs` | Initialization progress tracking |
| **Git Auto-Init** | `workspace_state.rs` | New workspace git repo creation |
| **Access Tracking** | `db.rs` | `last_accessed_at` auto-update on read |
| **Fragmentation Stats** | `db.rs` | `DbStats::fragmentation_percent()` |
| **Session Compaction** | `manager.rs` | Extract facts from old messages to memories |

---

## OpenClaw-Exclusive Memory Features (Not in Manta)

| Feature | Module | Gap |
|---------|--------|-----|
| **Multiple Backends** | `memory/` | Manta only supports SQLite; OpenClaw has QMD, LanceDB |
| **Session Files** | `session-files.ts` | File-based session persistence outside DB |
| **Transcript System** | `transcripts.ts` | Rich transcript formatting and export |
| **Backend-Specific Batching** | `embedding-*` | Gemini/OpenAI/Voyage native batch APIs |
| **LanceDB Support** | `memory/` | Columnar vector DB with filtering |
| **QMD (QuickMD) Support** | `memory/` | OpenClaw's custom vector format |

---

## File Mapping

| OpenClaw File | Manta File | Lines |
|---------------|------------|-------|
| `memory/` (~10,000 lines) | `src/memory/mod.rs` | ~434 |
| `memory/embedding-chunk-limits.ts` | `src/memory/vector.rs` (TextChunker) | ~906 |
| `memory/builtin/` | `src/memory/db.rs` (DatabaseStore) | ~1,239 |
| `memory/qmd/` | N/A | — |
| `memory/lancedb/` | N/A | — |
| `transcripts.ts` | `src/memory/db.rs` (ChatHistoryStore) | ~1,239 |
| `session-files.ts` | N/A | — |
| `agents/SOUL.md` etc. | `src/memory/personality.rs` | ~1,252 |
| N/A | `src/memory/manager.rs` | ~635 |
| N/A | `src/memory/hybrid.rs` | ~814 |
| N/A | `src/memory/session_search.rs` | ~680 |
| N/A | `src/memory/local_embeddings.rs` | ~395 |
| N/A | `src/memory/pipeline.rs` | ~336 |
| N/A | `src/memory/flush.rs` | ~271 |
| N/A | `src/memory/workspace_state.rs` | ~512 |
| N/A | `src/memory/sqlite.rs` | ~8 (shim) |

**Total**: OpenClaw ~10,000+ lines (TypeScript) vs Manta ~7,500+ lines (Rust) across memory-related files.

---

## Summary

Manta's memory system is **functionally equivalent** to OpenClaw's with several enhancements:

1. **Unified Backend**: Single SQLite database handles chat history, semantic memory, vector search, and FTS5 — no external dependencies
2. **Hybrid Search**: Vector + FTS5 with MMR re-ranking, temporal decay, and deduplication
3. **Local Embeddings**: On-device GGUF embedding models via llama-cpp-2
4. **Embedding Pipeline**: Background batched processing for throughput
5. **Unified Orchestrator**: `MemoryManager` coordinates all memory subsystems
6. **Session Context**: Single-call retrieval of episodic + semantic context
7. **Personality Memory**: 8 file types with budget enforcement, security scanning, and caching
8. **Workspace State**: Initialization progress tracking with git auto-init

The remaining ~15% gap is primarily in:
- **Multiple vector DB backends** (QMD, LanceDB) — Manta only supports SQLite
- **Session files** (file-based persistence outside DB)
- **Rich transcripts** (formatting and export)
- **Backend-specific batch APIs** (Gemini, Voyage native batching)
