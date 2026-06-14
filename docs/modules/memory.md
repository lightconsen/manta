# Memory Module

Persistent storage for conversations, messages, and memories with semantic search, tiered routing, and background dreaming.

## Design

### Storage Traits

```rust
#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn store(&self, memory: Memory) -> Result<MemoryId>;
    async fn get(&self, id: &MemoryId) -> Result<Option<Memory>>;
    async fn update(&self, memory: Memory) -> Result<()>;
    async fn delete(&self, id: &MemoryId) -> Result<bool>;
    async fn search(&self, query: MemoryQuery) -> Result<Vec<Memory>>;
    async fn cleanup_expired(&self) -> Result<usize>;
    async fn stats(&self) -> Result<MemoryStats>;
    async fn close(&self) -> Result<()>;
    fn as_tiered_store(&self) -> Option<&TieredStore>;
}

#[async_trait]
pub trait ChatHistoryStore: Send + Sync {
    async fn store_message(&self, message: ChatMessage) -> Result<()>;
    async fn get_conversation_history(&self, conversation_id: &str, limit: usize) -> Result<Vec<ChatMessage>>;
    async fn get_user_conversations(&self, user_id: &str, limit: usize) -> Result<Vec<String>>;
    async fn delete_conversation(&self, conversation_id: &str) -> Result<()>;
    async fn get_last_conversation(&self, user_id: &str) -> Result<Option<String>>;
}
```

### Backends

| Backend | Tier | Storage |
|---------|------|---------|
| `InMemoryStore` | Working | Ephemeral HashMap |
| `DatabaseStore` | ShortTerm, LongTerm | SQLite (sqlx) with WAL + FTS5 |
| `CompressedJsonlStore` | Archival | Gzip-compressed JSON Lines |

### Tiered Store

`TieredStore` routes each memory to its tier-specific backend based on `TierEvaluator::entry_tier()`. A `TierIndex` tracks which tier holds each memory ID for fast lookups.

- **Promotion**: Working → ShortTerm → LongTerm → Archival
- **Demotion**: Archival → LongTerm → ShortTerm → Working
- **Eviction**: When TTL expires or tier is disabled

### Memory Manager

`MemoryManager` is the high-level facade:
- `store: Arc<dyn MemoryStore>` — tiered or unified
- `chat_history: Arc<dyn ChatHistoryStore>` — always SQLite-backed
- `session_context()` — builds `SessionContext` with recent messages, retrieved memories, and multimodal references
- `retrieve()` — semantic search with optional embedding; also queries QMD executor (if available) and merges results

### Dreaming

`DreamScheduler` runs background cycles on a cron schedule:
- **Light Dream** — deduplication (embedding cosine similarity > threshold, fallback to text hash), tag cleanup, expired cleanup. Records `PromotionApplied` events on tier changes.
- **Deep Dream** — topic clustering, summary generation
- **REM Dream** — cross-session association, pattern discovery, knowledge graph update. Graph is persisted to `memory/.dreams/knowledge_graph.json`.

Started in `Gateway::start()` with `event_log` and `workspace_dir` wired in.

### Event System

`MemoryEventLog` writes JSONL to `memory/.dreams/events.jsonl`:
- `RecallRecorded` — when memories are recalled into session context
- `PromotionApplied` — when memories are promoted/demoted between tiers
- `CompactCompleted` — when a session is compacted into semantic memories
- `DreamCompleted` — when a dream phase finishes

### Subsystems

- **Multimodal** (`multimodal.rs`) — File classification (image/audio), glob scanning, path management
- **Events** (`events.rs`) — JSONL event log for recall, promotion, compact, and dream events
- **QMD** (`qmd.rs`) — Query Markdown/Document CLI wrapper with `QmdScope` (channel, chat_type, key_prefix, allow/deny) access control; wired into `retrieve()`
- **Vector** (`vector.rs`) — Embedding providers (API, cached, local GGUF), text chunking, vector stores
- **Hybrid Search** (`hybrid.rs`) — Combines semantic + keyword with MMR rerank and temporal decay
- **Effectiveness** (`effectiveness.rs`) — Tracks recall hit rates to tune memory importance
- **Session Search** (`session_search.rs`) — Search across conversation history

## Missing / TODO

- **✅ Implemented**: Effectiveness tracker — `MemoryManager::new()` creates an `EffectivenessTracker` and `retrieve()` calls `record_recall()` for every recalled memory. See `src/memory/manager.rs:149-153` and `src/memory/manager.rs:448-460`.
- **✅ Implemented**: Effectiveness closed-loop feedback — `EffectivenessTracker` hit-rate stats are fed into `TierEvaluator::evaluate()` via optional `EffectivenessStats`. `MemoryManager::apply_effectiveness_adjustments()` updates importance scores and explicitly migrates memories between tiers when effectiveness thresholds are met, emitting `PromotionApplied` events. See `src/memory/tier.rs:247-295` and `src/memory/manager.rs:889-1024`.
- **✅ Implemented**: Local embeddings (`local-embeddings` feature) — `ModelSource` parsing, local path resolution, and FTS-only fallback are covered by dedicated unit tests in `src/memory/local_embeddings.rs`. A dedicated CI job (`test-local-embeddings`) runs `cargo test --features local-embeddings` on every push/PR.
- **✅ Implemented**: Vector store backend abstraction (pgvector, sqlite-vec) — `PgVectorStore` and `SqliteVecStore` implement the `VectorStore` trait behind feature gates `pgvector` and `sqlite-vec`. `VectorBackend` now includes `Postgres` and `SqliteVec` variants. See `src/memory/pgvector_store.rs` and `src/memory/sqlite_vec_store.rs`.
- **✅ Implemented**: Memory export/import for migration — `ExportService` now supports `import_memories`, `import_conversations`, and `import_all` with `ImportOptions` for skip/update/dry-run semantics. JSON and JSONL formats are supported, and records are validated before insertion. See `src/export/service.rs:345-650`.
- **✅ Implemented**: Soul/personality file auto-generation — `PersonalityMemory::analyze_conversation_patterns()` heuristically detects language, code style, voice/tone, common topics, and explicit preferences from conversation history. `SoulConfig::merge_analysis()` fills empty SOUL.md fields conservatively, and `MemoryManager::compact_session()` auto-updates SOUL.md after compaction. See `src/memory/personality.rs:410-540` and `src/memory/manager.rs:715-730`.
- **✅ Implemented**: Dream result human review — `DreamReviewQueue` with `enqueue()`/`approve()`/`reject()`/`list_pending()` and disk persistence, wired into `DreamEngine` via optional `review_queue`. See `src/memory/dreaming.rs:1093-1150`.
- **✅ Implemented**: Dream observability dashboard — `DreamResult` now includes `duration_ms`, `peak_memory_mb`, `llm_tokens_input`, and `llm_tokens_output`. `DreamMetrics` tracks cumulative counters for dreams, memory operations, duration, and tokens. Metrics are exposed via the Prometheus `/metrics` endpoint and the `/health` JSON report. See `src/memory/dreaming.rs:119-207` and `src/gateway/handlers/health.rs`.
