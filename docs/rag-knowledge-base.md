# RAG Knowledge Base Design

Per-agent knowledge base system: assign independent document collections to each agent, with automated ingestion and collection-scoped retrieval.

## 1. Data Model

### KnowledgeSource

A source definition — where documents come from.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeSource {
    /// Unique ID within an agent (auto-generated if not specified)
    pub id: Option<String>,
    /// Source type
    pub source_type: SourceType,
    /// Optional glob pattern for file/dir sources (e.g. "**/*.md")
    pub pattern: Option<String>,
    /// Optional collection override (default: "kb-{agent_id}")
    pub collection: Option<String>,
    /// Chunk strategy override (default: agent-level or global default)
    pub chunk_strategy: Option<ChunkStrategy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SourceType {
    /// Single file
    File { path: String },
    /// Recursive directory
    Dir { path: String },
    /// Remote URL (HTML/markdown)
    Url { url: String },
    /// Glob pattern relative to agent dir
    Glob { pattern: String },
}
```

### KnowledgeDocument

A loaded document, after source resolution.

```rust
pub struct KnowledgeDocument {
    /// Source that produced this document
    pub source_id: String,
    /// Logical document ID (file path / URL)
    pub doc_id: String,
    /// Raw content
    pub content: String,
    /// File metadata (for change detection)
    pub checksum: Option<String>,
    pub mtime: Option<i64>,
    /// Content type hint
    pub mime_type: Option<String>,
}
```

### IngestionRecord

Tracks what has been ingested (stored in SQLite).

```rust
pub struct IngestionRecord {
    pub collection: String,
    pub doc_id: String,
    pub source_id: String,
    pub checksum: String,
    pub mtime: i64,
    pub chunk_count: usize,
    pub status: IngestionStatus,
    pub indexed_at: chrono::DateTime<Utc>,
    pub error: Option<String>,
}

pub enum IngestionStatus {
    Indexed,
    Failed,
    Stale,   // source changed since last index
}
```

## 2. Configuration

### Per-Agent KB Config (`~/.syscity/agents/{agent-id}/kb.toml`)

Co-located with the agent's personality files:

```toml
# ── Knowledge base sources ─────────────────────────────────
[[sources]]
type = "dir"
path = "./docs"
pattern = "**/*.md"

[[sources]]
type = "url"
url = "https://wiki.internal/runbooks"

[[sources]]
type = "file"
path = "./SOP.pdf"

# ── Per-agent overrides (optional) ─────────────────────────
chunk_size = 512
collection = "kb-sre"        # default: "kb-{agent_id}"
```

### Global Config Extension (`syscity.toml`)

```toml
[agents.sre]
personality_dir = "~/.syscity/agents/sre"
# knowledge_base sources can also be inlined here
knowledge_base = [
    { type = "dir", path = "./docs/sre", pattern = "**/*.md" },
]
```

Resolution order: `syscity.toml` `[agents.<id>]` sources merge with `kb.toml` sources (kb.toml wins on conflicts).

## 3. Ingestion Pipeline

```
Source config
    │
    ▼
┌─────────────────────────────────────┐
│        Source Loader                │
│  ─ file loader (read + checksum)    │
│  ─ dir loader (glob, recursive)     │
│  ─ URL loader (HTTP fetch)          │
│  ─ stdin / pipe                     │
└────────────┬────────────────────────┘
             │  Vec<KnowledgeDocument>
             ▼
┌─────────────────────────────────────┐
│     Change Detection                │
│  ─ compare checksum/mtime vs        │
│    IngestionRecord                  │
│  ─ skip unchanged docs              │
└────────────┬────────────────────────┘
             │  new/changed docs only
             ▼
┌─────────────────────────────────────┐
│        TextChunker                  │
│  ─ existing recursive chunking      │
│  ─ per-source override support      │
└────────────┬────────────────────────┘
             │  Vec<String> chunks
             ▼
┌─────────────────────────────────────┐
│     BatchEmbeddingProcessor         │
│  ─ embed in batches (existing)      │
│  ─ set collection = "kb-{agent_id}" │
└────────────┬────────────────────────┘
             │  Vec<EmbeddedChunk>
             ▼
┌─────────────────────────────────────┐
│        VectorStore.store_chunks()   │
│  ─ delete stale chunks for doc_id   │
│  ─ insert new chunks                │
│  ─ update IngestionRecord           │
└─────────────────────────────────────┘
```

### Change Detection

On re-ingest:
1. For each source, list current files
2. Compute checksum (SHA256 of first 4KB + file size) or mtime
3. Query `kb_ingestion_log` for existing records
4. Skip docs where checksum and mtime match
5. For changed docs: delete old chunks (`delete_by_source()`) then re-index
6. For removed files: delete their chunks and mark records as stale

### Supported Formats

| Format | Source | Loader |
|--------|--------|--------|
| Markdown | file/dir/glob | Direct read |
| Plain text | file/dir/glob | Direct read |
| PDF | file/dir/glob | `pdf-extract` or `lopdf` |
| HTML | URL | `reqwest` + html-to-text |
| Code files | file/dir/glob | Read as text |

Phase 1: Markdown + plain text + code. PDF and URL in Phase 2.

## 4. Storage

### New SQLite Table: `kb_ingestion_log`

```sql
CREATE TABLE IF NOT EXISTS kb_ingestion_log (
    collection TEXT NOT NULL,
    doc_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    checksum TEXT,
    mtime INTEGER,
    chunk_count INTEGER DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'indexed',   -- indexed, failed, stale
    error TEXT,
    indexed_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (collection, doc_id)
);
```

### Collection Naming Convention

- Default: `kb-{agent_id}` (e.g. `kb-sre`, `kb-coder`)
- Overridable per source via `collection` field

### Storage Location

The ingestion log goes into the existing SQLite database (`~/.syscity/data/syscity.db`), same as the vector store. A new `DatabaseStore` method `ensure_kb_tables()` initializes the schema.

## 5. Retrieval Integration

### Current Flow

```
Message → Router (resolves agent_id) → Dispatch → Agent.process_message()
                                                          │
                                                    MemoryManager.retrieve()
                                                          │
                                              hybrid_search(query, user_id, conv_id, vs, ss, cfg)
                                                          │
                                            vector_service.search(query, limit, threshold)
                                                          │  (collection = None = all)
                                            VectorStore.search_similar(embedding, limit, threshold, None)
```

### Proposed Flow

```
Message → Router (resolves agent_id) → Dispatch → Agent.process_message()
                                                          │
                                                    MemoryManager.retrieve()
                                                          │
                                              hybrid_search(query, user_id, conv_id, vs, ss, cfg)
                                                          │
                                            vector_service.search_collection(
                                                query, limit, "kb-{agent_id}", threshold
                                            )
                                                          │
                                            VectorStore.search_similar(embedding, limit, threshold, Some("kb-{agent_id}"))
```

### Changes Required

1. **`hybrid_search()`** — accept optional `kb_collection: Option<&str>` parameter. Pass it to `vector_service.search_collection()` when set.

2. **`MemoryManager::retrieve()`** — accept optional `kb_collection: Option<String>` parameter. Thread it to `hybrid_search()`.

3. **`MemoryManager::session_context()`** — already receives `conversation_id` and `query`. Add `kb_collection: Option<String>` parameter.

4. **Agent process path** — the agent's `process_message()` or `MemoryManager` caller needs to know the agent's identity to derive the kb collection name. Options:
   - Store `agent_id` on the `MemoryManager` instance
   - Pass it at call time from the agent handle

   Recommended: store `agent_id` on `MemoryManagerConfig` (default `None`). When set, derive `kb_collection = Some(format!("kb-{}", agent_id))` automatically. The agent identity is already available at init time.

### What About the Global Vector Memory?

The existing `vector_memory` (no collection) remains for:
- Cross-agent knowledge (shared docs)
- The `MemoryManager` dreaming/consolidation pipeline
- User's personal semantic memory

The kb collection search is *additive* — results from the agent's kb are injected alongside existing memory results. The full retrieval becomes:

```
hybrid_search(query, kb_collection)  → kb results
hybrid_search(query, None)           → global memory results
    ↓
merge → deduplicate (by content hash) → rerank → context window budget
```

## 6. CLI

### Top-Level Command

```bash
syscity kb <subcommand> [options]
```

Registered as a new `Commands` variant in `src/cli/mod.rs`:

```rust
/// Knowledge base management
#[command(name = "kb")]
Kb(KbArgs),
```

### Subcommands

#### `ingest` — Ingest sources into a knowledge base

```bash
syscity kb ingest --agent sre
syscity kb ingest --agent sre --source ./docs/runbook.md  # ad-hoc, skip config
syscity kb ingest --agent sre --source https://wiki.internal/runbooks
syscity kb ingest --all     # ingest all configured agents
syscity kb ingest --agent sre --rebuild   # force re-ingest all
```

#### `list` — List ingested documents

```bash
syscity kb list
syscity kb list --agent sre
syscity kb list --agent sre --status stale
```

Output:
```
Collection    Doc ID                               Chunks   Status   Indexed At
kb-sre        docs/runbook.md                      24       indexed  2026-07-24 10:00
kb-sre        docs/sop.md                          12       indexed  2026-07-24 10:00
kb-coder      docs/api-reference.md                48       indexed  2026-07-24 09:30
```

#### `delete` — Delete documents or entire collections

```bash
syscity kb delete --agent sre
syscity kb delete --agent sre --doc docs/old.md
syscity kb delete --collection kb-sre
```

#### `watch` — Watch directories and auto-reindex

```bash
syscity kb watch --agent sre
syscity kb watch --all
```

Uses `notify` crate (file system events) to detect changes and trigger incremental re-ingest. Runs as a foreground process (or integrates with the daemon's runtime).

#### `status` — Knowledge base health

```bash
syscity kb status --agent sre
```

Shows: total docs, total chunks, last index time, stale count, failed count.

### Architecture for CLI Commands

All CLI commands follow the existing pattern:
1. Parse args
2. Load gateway config
3. Initialize the database pool (SQLite)
4. Create a `KnowledgeBaseManager` instance
5. Execute the operation

For `ingest`, the manager:
1. Reads `kb.toml` from the agent directory
2. Loads source documents
3. Creates the embedding provider (from gateway config)
4. Processes through the ingestion pipeline
5. Stores chunks + updates `kb_ingestion_log`

For `watch`, uses `notify::RecommendedWatcher` and re-triggers ingestion on file change events.

## 7. New Modules

```
src/
  rag/
    ingestion/
      mod.rs         — KnowledgeBaseManager, public API
      loader.rs      — Source loaders (file, dir, url, glob)
      tracker.rs     — IngestionRecord + kb_ingestion_log
      watch.rs       — File watcher for auto-reindex
  cli/
    kb.rs            — CLI subcommand handler
```

### KnowledgeBaseManager Public API

```rust
pub struct KnowledgeBaseManager {
    embedding_provider: Arc<dyn EmbeddingProvider>,
    vector_store: Arc<dyn VectorStore>,
    store: Arc<DatabaseStore>,  // for ingestion log
    chunker: TextChunker,
    batch_size: usize,
}

impl KnowledgeBaseManager {
    /// Create from gateway config
    pub async fn from_config(config: &GatewayConfig) -> crate::Result<Self>;

    /// Ingest all sources for an agent
    pub async fn ingest_agent(
        &self,
        agent_id: &str,
        force_rebuild: bool,
    ) -> crate::Result<IngestReport>;

    /// Ingest a single ad-hoc source
    pub async fn ingest_source(
        &self,
        collection: &str,
        source: KnowledgeSource,
        force: bool,
    ) -> crate::Result<IngestReport>;

    /// List ingested records
    pub async fn list(
        &self,
        collection: Option<&str>,
        status: Option<IngestionStatus>,
    ) -> crate::Result<Vec<IngestionRecord>>;

    /// Delete collection or specific docs
    pub async fn delete(
        &self,
        collection: &str,
        doc_id: Option<&str>,
    ) -> crate::Result<DeleteReport>;

    /// Watch for changes and auto-reindex
    pub async fn watch(
        &self,
        agent_id: Option<&str>,
    ) -> crate::Result<()>;
}
```

### IngestReport

```rust
pub struct IngestReport {
    pub collection: String,
    pub total_sources: usize,
    pub docs_found: usize,
    pub docs_indexed: usize,     // new/changed
    pub docs_skipped: usize,     // unchanged
    pub total_chunks: usize,
    pub errors: Vec<String>,
    pub duration: std::time::Duration,
}
```

## 8. Runtime Integration

### Daemon Mode

When the daemon (`syscity start`) is running:

1. **On startup**: Optionally auto-ingest all configured agents (configurable via `auto_ingest_on_start = true` in `kb.toml` or global config)
2. **On agent discovery**: If `kb.toml` exists for a new agent, auto-ingest
3. **Watch mode**: Can run as a background task within the daemon (integration with existing `tokio::spawn` + `TaskRegistry`)
4. **Hot reload**: If `kb.toml` changes, trigger re-ingest

### Retrieval at Runtime

When an agent processes a message:

1. The `AgentHandle` has `agent_id` set
2. `AgentConfig` (or `MemoryManagerConfig`) carries `kb_collection: Option<String>`
3. `MemoryManager::retrieve()` passes `kb_collection` to `hybrid_search()`
4. Results from the kb are interleaved with global memory results

For the dispatch path, the `AgentRouter` already resolves `agent_id`. This is threaded through to the agent at spawn time:

```rust
// In dispatch.rs or gateway/mod.rs spawn path:
let agent_config = personality.to_agent_config_for(ctx);
agent_config.agent_id = Some(agent_id.clone());
agent_config.kb_collection = Some(format!("kb-{}", agent_id));
```

The `MemoryManager` is created with this config and automatically scopes retrieval.

## 9. File Changes Summary

| File | Action |
|------|--------|
| `src/rag/ingestion/mod.rs` | **New** — `KnowledgeBaseManager` |
| `src/rag/ingestion/loader.rs` | **New** — Source loaders |
| `src/rag/ingestion/tracker.rs` | **New** — Ingestion log |
| `src/rag/ingestion/watch.rs` | **New** — File watcher |
| `src/rag/mod.rs` | Add `pub mod ingestion;` |
| `src/cli/mod.rs` | Add `Kb(KbArgs)` variant |
| `src/cli/kb.rs` | **New** — CLI handler |
| `src/memory/manager.rs` | Add `kb_collection` to config + retrieval |
| `src/memory/hybrid.rs` | Accept `kb_collection` param in `hybrid_search()` |
| `src/agent/config.rs` or `src/agent/mod.rs` | Add `kb_collection: Option<String>` to `AgentConfig` |
| `src/gateway/dispatch.rs` | Set `kb_collection` on agent config at spawn |
| `src/rag/vector_store.rs` | No change (collection already supported) |
| `src/rag/sqlite_vec_store.rs` | No change (collection filtering already works) |
| `src/storage/database.rs` | Add `ensure_kb_tables()` |
| `~/.syscity/agents/{id}/kb.toml` | **New** — Per-agent KB config |
| `docs/config-guide.md` | Add KB config section |

## 10. 当前 RAG 实现状态（对照最佳实践）

Syscity 现有的 RAG 实现逐层分析，对照业界最佳实践。

### 架构总览

```
                          ┌─────────────────────┐
                          │    User Query        │
                          └──────────┬──────────┘
                                     │
                          ┌──────────▼──────────┐
                ┌─────────│   Query Rewriting   │─────────┐
                │         │  (HyDE / Multi-Query)│         │
                │         └──────────┬──────────┘         │
                │                    │                    │
         ┌──────▼──────┐    ┌───────▼───────┐    ┌───────▼──────┐
         │  Embedding  │    │   FTS5 (BM25) │    │  KB Sources  │
         │  (vector)   │    │  (keyword)    │    │  (pending)   │
         └──────┬──────┘    └───────┬───────┘    └───────┬──────┘
                │                    │                    │
         ┌──────▼────────────────────▼────────────────────▼──────┐
         │               Hybrid  Fusion (fuse_and_rerank)        │
         │         normalization → dedup → weighted merge        │
         └──────────────────────┬────────────────────────────────┘
                                │
         ┌──────────────────────▼────────────────────────────────┐
         │          Post-Retrieval Pipeline                      │
         │   temporal decay → MMR → Cross-encoder reranker      │
         └──────────────────────┬────────────────────────────────┘
                                │
         ┌──────────────────────▼────────────────────────────────┐
         │          Context Window Budget                        │
         │   token-aware truncation (min_chunks guarantee)       │
         └──────────────────────┬────────────────────────────────┘
                                │
                              LLM
```

### Layer 1 — Ingestion Pipeline（数据入库）

**最佳实践**: `unstructured.io` + langchain document-loader → chunk → embed

**现状**: ❌ 缺失

| 功能 | 状态 | 说明 |
|------|------|------|
| 文件导入 | ❌ | 没有文件/目录批量导入能力 |
| URL 抓取 | ❌ | 不支持从 URL 自动拉取文档 |
| PDF/Word 解析 | ❌ | 无文档格式解析器 |
| 增量更新 | ❌ | 无变更检测机制 |
| 来源追踪 | ❌ | 无 ingestion log |

现有唯一条目：`CLI memory add` 手动单条添加内容（`src/cli/memory.rs`）。本设计文档的完整 ingestion pipeline 即为解决此层的方案。

### Layer 2 — Chunking（分块策略）

**最佳实践**: RecursiveCharacterTextSplitter（层级分隔符递归拆分）+ semantic chunking

**现状**: ✅ 已完成

**文件**: `src/rag/chunk.rs`

```rust
pub enum ChunkStrategy {
    Fixed { chunk_size: usize, chunk_overlap: usize },
    Recursive { chunk_size: usize, separators: Option<Vec<String>> },
}
```

| 特性 | 状态 | 说明 |
|------|------|------|
| 定长滑动窗口 | ✅ | word-level 重叠分块，修复了 overlap≥size 无限循环 |
| 递归分块 | ✅ | 默认策略，按 `\n\n` → `\n` → `. ` → ` ` 优先级拆分 |
| 默认改为递归 | ✅ | 从 Fixed 改为了 Recursive { chunk_size: 512 } |
| 异步接口 | ✅ | `chunk_async()` 通过 `spawn_blocking` 避免阻塞 tokio |
| Semantic chunking | ❌ | 用 embedding 检测语义边界再切，尚未实现 |

### Layer 3 — Embedding（向量化）

**最佳实践**: `text-embedding-3-small` / `voyage-2` / `bge-m3`

**现状**: ✅ 已完成

**文件**: `src/rag/embedding.rs`, `src/rag/local_embeddings.rs`

| 特性 | 状态 | 说明 |
|------|------|------|
| API 嵌入 | ✅ | `ApiEmbeddingProvider` — OpenAI 兼容接口，可配 base_url（兼容 Azure） |
| 本地 GGUF | ✅ | `LocalGgufEmbeddingProvider` — llama.cpp 推理，HF Hub 自动下载 |
| Embedding 缓存 | ✅ | `CachedEmbeddingProvider` — SHA-256 内容去重，FIFO 淘汰 |
| 懒加载 | ✅ | GGUF 模型在首次使用时才加载，不阻塞启动 |
| FTS-only 降级 | ✅ | GGUF 模型不可用时自动降级为纯关键词搜索 |

### Layer 4 — Index（向量索引）

**最佳实践**: Hybrid 索引（稠密 + 稀疏），PGVector / Qdrant / Milvus

**现状**: ✅ 已完成（但 hybrid 在检索层而非索引层）

**文件**: `src/rag/vector_store.rs`, `src/rag/sqlite_vec_store.rs`, `src/rag/pgvector_store.rs`

| 后端 | 状态 | 技术 |
|------|------|------|
| `MemoryVectorStore` | ✅ | HashMap + 线性扫描，FIFO 淘汰（默认 10 万条），开发/测试用 |
| `SqliteVecStore` | ✅ | `sqlite-vec` 虚拟表 + cosine 距离，`vec_chunk_collections` JOIN 表实现 collection 过滤 |
| `PgVectorStore` | ✅ | PostgreSQL pgvector 扩展 + `<=>` 余弦距离算符 |

| 特性 | 状态 | 说明 |
|------|------|------|
| Collection 隔离 | ✅ | 每个 chunk 可标记 collection，搜索时 JOIN 过滤 |
| Cosine 相似度 | ✅ | 三个后端均支持 |
| 阈值过滤 | ✅ | distance 阈值 |
| 原生混合索引 | ⚠️ | 单索引只有 vector ANN。FTS5 在独立的 `session_messages` 表中；hybrid 由上层 `fuse_and_rerank()` 手动融合 |

### Layer 5 — Retrieval Pipeline（检索流程）

**最佳实践**: Hybrid → Rerank → Context window budget

**现状**: ✅ 完整链路

**文件**: `src/rag/hybrid.rs`, `src/memory/hybrid.rs`, `src/rag/context.rs`, `src/rag/reranker.rs`

```
vector search ──┐
                ├──→ fuse_and_rerank() → temporal_decay → MMR → reranker → context_budget
FTS5 search ────┘
```

| 阶段 | 状态 | 说明 |
|------|------|------|
| 并行检索 | ✅ | `tokio::join!` 同时跑 vector + FTS |
| Score 归一化 | ✅ | min-max normalization |
| 内容去重 | ✅ | content hash + source_id 双层去重 |
| 加权融合 | ✅ | 可配 vector/FTS 权重（默认 0.7 / 0.3） |
| 时间衰减 | ✅ | 指数衰减，`half_life_days` 可配 |
| MMR 重排 | ✅ | Jaccard-word 多样性，`lambda` / `top_k` 可配 |
| Cross-encoder 重排 | ✅ | `CohereReranker`（rerank-english-v3.0） |
| Token 预算裁剪 | ✅ | greedy 按 score 截断，min_chunks 兜底 |
| 检索质量评估 | ✅ | `evaluate_retrieval()` — recall@k / MRR@k / hit_rate@k |

### Layer 6 — Query Rewriting（查询改写）

**最佳实践**: HyDE + Multi-Query（多子查询并行检索后合并）

**现状**: ✅ 完整（HyDE + Multi-Query 均已实现）

**文件**: `src/rag/query.rs`, `src/memory/query.rs`, `src/rag/multi_query.rs`

```rust
#[async_trait]
pub trait QueryTransformer: Send + Sync {
    async fn transform(&self, query: &str) -> crate::Result<String>;
}
```

| 实现 | 状态 | 说明 |
|------|------|------|
| `NoopTransformer` | ✅ | 透传，默认 |
| `HydeTransformer` | ✅ | LLM 先生成"假设的理想回答"，再用这段去嵌入（提升 recall） |
| `expand_query_with_llm` + RRF | ✅ | Multi-Query：LLM 扩展为 N 个子查询 → 并行检索 → RRF 合并 |

HyDE 和 Multi-Query 的 LLM provider 均在 `gateway/init/services.rs` 中注入。可通过 `query_transformer.enable_hyde = true` 和 `multi_query.enabled = true` 分别启用。

### Layer 7 — Advanced RAG（进阶方案）

**最佳实践**: GraphRAG / Agentic RAG / Self-RAG

**现状**: ⚠️ 部分（轻量 KG 但未用于检索）

| 方案 | 状态 | 说明 |
|------|------|------|
| GraphRAG | ⚠️ | dreaming engine 有轻量 KG（实体抽取 + JSON 持久化），但仅用于记忆整合，不做图遍历检索 |
| Agentic RAG | ❌ | 无 tool-call 按需检索模式（当前是每次注入） |
| Self-RAG | ❌ | 无检索后 LLM 自评相关性的机制 |
| Multi-hop 检索 | ❌ | 无问题分解 → 多步检索 → 综合答案的流程 |
| Query routing | ❌ | 无根据问题类型选择索引/collection 的路由器 |

### 总结

| 层次 | 评分 | 优先级 |
|------|------|--------|
| Ingestion Pipeline | ❌ 缺失（本设计方案解决） | **高** — 知识库方案第一步 |
| Chunking | ✅ 完整 | — |
| Embedding | ✅ 完整 | — |
| Index | ✅ 完整 | — |
| Retrieval | ✅ 完整 | — |
| Query Rewriting | ✅ 完整 | — |
| Advanced | ⚠️ 缺多项 | 低 — 需要具体场景驱动 |

核心检索链路（Layer 2-6）已到工业级水准；最大短板是文档入库（Layer 1），即本设计文档的核心目标。

---

## 11. Implementation Phases

### Phase 1 — Core (estimate: 3-5 days)
- `kb.toml` config loading
- File/dir source loaders (markdown + text + code)
- Ingestion pipeline (load → chunk → embed → store)
- `kb_ingestion_log` table + tracking
- CLI: `kb ingest`, `kb list`, `kb delete`

### Phase 2 — Retrieval Integration (estimate: 2-3 days)
- Thread `kb_collection` through `MemoryManager`
- Scoped `hybrid_search()` with collection
- Merge kb results + global memory results

### Phase 3 — Watch & Auto (estimate: 2 days)
- File watcher with `notify` crate
- Daemon auto-ingest on startup
- Hot reload support

### Phase 4 — Advanced Sources (estimate: 2-3 days)
- URL loader (HTTP fetch + HTML-to-text)
- PDF support
- Glob patterns
- Ad-hoc ingestion via CLI `--source`
