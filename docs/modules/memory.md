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
- `retrieve()` — semantic search with optional embedding

### Dreaming

`DreamScheduler` runs background cycles on a cron schedule:
- **Light Dream** — deduplication, tag cleanup, expired cleanup
- **Deep Dream** — topic clustering, summary generation
- **REM Dream** — cross-session association, pattern discovery

Started in `Gateway::start()`.

### Subsystems

- **Multimodal** (`multimodal.rs`) — File classification (image/audio), glob scanning, path management
- **Events** (`events.rs`) — JSONL event log for recall, promotion, and dream events
- **QMD** (`qmd.rs`) — Query Markdown/Document CLI wrapper for semantic document search
- **Vector** (`vector.rs`) — Embedding providers (API, cached, local GGUF), text chunking, vector stores
- **Hybrid Search** (`hybrid.rs`) — Combines semantic + keyword with MMR rerank and temporal decay
- **Effectiveness** (`effectiveness.rs`) — Tracks recall hit rates to tune memory importance
- **Session Search** (`session_search.rs`) — Search across conversation history

## Missing / TODO

- **Partial**: REM Dream cross-session association and knowledge graph are stubbed but not fully implemented.
- **Missing**: Effectiveness tracker is not yet wired into the memory manager feedback loop.
- **Missing**: QMD scope-based access control (channel/chatType/keyPrefix filtering) is not enforced in queries.
- **Missing**: Local embeddings (`local-embeddings` feature) exists but is feature-gated and not validated in CI.
- **Missing**: Vector store backend abstraction (pgvector, sqlite-vec) — only SQLite FTS5 + in-memory embeddings currently.
- **Missing**: Memory export/import for migration between workspaces.
- **Missing**: Soul/personality file auto-generation from conversation patterns.
