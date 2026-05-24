# Issue: OpenClaw Memory 层的复杂机制

## 背景

Manta 当前在 `src/memory/` 中使用 SQLite (sqlx) 作为核心存储，可选 pgvector 作为向量数据库，本地 embeddings 处理语义搜索。功能涵盖 soul、personality、session search、workspace state 等。OpenClaw 的 Memory 层在相似的技术栈（sqlite-vec + LanceDB）之上，构建了一套完整的多模态存储、QMD 查询、事件日志和 Dreaming 机制，使记忆系统不仅存储文本，还能管理文件、支持结构化向量查询、记录记忆事件，并在后台自动进行记忆整理。

---

## 1. Multimodal Storage — 多模态文件存储

### 文件类型规范

`src/memory-host-sdk/host/multimodal.ts` 定义了内存中多模态文件的完整规范：

```typescript
const MEMORY_MULTIMODAL_SPECS = {
  image: {
    labelPrefix: "Image file",
    extensions: [".jpg", ".jpeg", ".png", ".webp", ".gif", ".heic", ".heif"],
  },
  audio: {
    labelPrefix: "Audio file",
    extensions: [".mp3", ".wav", ".ogg", ".opus", ".m4a", ".aac", ".flac"],
  },
};
```

**支持的格式**：
- **Image**: JPEG, PNG, WebP, GIF, HEIC, HEIF（7 种格式）
- **Audio**: MP3, WAV, OGG, Opus, M4A, AAC, FLAC（7 种格式）

### 配置模型

```typescript
export type MemoryMultimodalModality = "image" | "audio";

export type MemoryMultimodalSettings = {
  enabled: boolean;
  modalities: MemoryMultimodalModality[];
  maxFileBytes: number;
};

export const DEFAULT_MEMORY_MULTIMODAL_MAX_FILE_BYTES = 10 * 1024 * 1024; // 10MB
```

- `enabled` — 是否启用多模态存储
- `modalities` — 启用的模态类型（可单独启用 image 或 audio）
- `maxFileBytes` — 单个文件大小上限（默认 10MB）

### 文件分类与路径处理

```typescript
export function classifyMemoryMultimodalFile(
  filePath: string,
  settings: MemoryMultimodalSettings,
): { modality: MemoryMultimodalModality; extension: string } | null;

export function buildMemoryMultimodalGlob(modality: MemoryMultimodalModality): string;
```

**分类逻辑**：
1. 提取文件扩展名（不区分大小写）
2. 对照 `MEMORY_MULTIMODAL_SPECS` 匹配所属模态
3. 检查该模态是否在 `settings.modalities` 中启用
4. 检查文件大小是否超过 `maxFileBytes`

**Glob 构建**：为每种模态生成大小写不敏感的通配符模式，用于文件系统扫描。

### 存储策略

多模态文件存储在 workspace 的 `memory/` 目录下，按模态分类：
- `memory/images/` — 图像文件
- `memory/audio/` — 音频文件

文件通过 `labelPrefix` + 文件名生成人类可读的标签，供 LLM 引用。

---

## 2. QMD Query — QMD 向量查询系统

QMD（Query Markdown/Document）是 OpenClaw 用于在记忆文档中进行语义/结构化查询的机制，基于外部 `qmd` CLI 工具实现。

### 查询结果结构

`src/memory-host-sdk/host/qmd-query-parser.ts` 定义了查询返回的数据结构：

```typescript
export type QmdQueryResult = {
  docid?: string;        // 文档唯一 ID
  score?: number;        // 匹配分数
  collection?: string;   // 所属集合
  file?: string;         // 源文件路径
  snippet?: string;      // 匹配片段摘要
  body?: string;         // 完整内容
  startLine?: number;    // 起始行号
  endLine?: number;      // 结束行号
};
```

### 基于 Scope 的访问控制

`src/memory-host-sdk/host/qmd-scope.ts` 实现了细粒度的查询范围控制：

```typescript
export type ResolvedQmdConfig = {
  scope: {
    channel?: string;     // 渠道限制（如 "telegram", "discord"）
    chatType?: string;    // 聊天类型（"direct" | "group"）
    keyPrefix?: string;   // 会话 key 前缀
    allow?: string[];     // 显式允许的标识列表
    deny?: string[];      // 显式拒绝的标识列表
  };
};

export function isQmdScopeAllowed(
  scope: ResolvedQmdConfig["scope"],
  sessionKey?: string,
): boolean;
```

**权限决策逻辑**：
1. **Channel 匹配**：检查查询请求的来源渠道是否在 scope 限制内
2. **ChatType 匹配**：区分私聊/群聊的查询权限
3. **KeyPrefix 匹配**：按会话 key 前缀过滤（如 `telegram:12345:`）
4. **Allow/Deny 列表**：显式允许/拒绝特定标识
5. 任何一层不匹配即拒绝查询

### CLI 执行包装

`src/memory-host-sdk/host/qmd-process.ts` 提供了 `qmd` 二进制文件的执行封装：

```typescript
export async function isQmdAvailable(): Promise<boolean>;
// 检查 qmd 是否在 PATH 中可用

export async function runQmdQuery(
  query: string,
  options: {
    cwd?: string;
    timeout?: number;
    scope?: ResolvedQmdConfig["scope"];
  },
): Promise<QmdQueryResult[]>;
```

**执行流程**：
1. 检查 `qmd` 命令是否可用（`isQmdAvailable`）
2. 构建查询参数（query string + scope filter）
3. 通过 `spawn` 执行 `qmd` CLI，支持超时控制
4. 解析 JSON 输出为 `QmdQueryResult[]`
5. 按 score 排序返回结果

### 查询使用场景

- **会话上下文召回**：根据当前会话主题查询相关历史记忆
- **跨会话关联**：通过向量相似度找到其他会话中的相关内容
- **文档检索**：在 workspace 的记忆文档中查找特定信息
- **安全隔离**：确保用户只能查询到自己有权限访问的记忆

---

## 3. Event System — 记忆事件系统

### 事件类型定义

`src/memory-host-sdk/events.ts` 实现了基于 JSONL 的记忆事件日志系统：

```typescript
export type MemoryHostEvent =
  | MemoryHostRecallRecordedEvent      // 记忆召回事件
  | MemoryHostPromotionAppliedEvent    // 记忆提升事件
  | MemoryHostDreamCompletedEvent;     // Dream 完成事件
```

### 三种核心事件

#### 1. Recall Recorded（记忆召回）

```typescript
type MemoryHostRecallRecordedEvent = {
  type: "memory.recall.recorded";
  timestamp: number;
  sessionKey: string;
  recallId: string;
  source: string;           // 召回来源
  contentSummary: string;   // 内容摘要
};
```

当系统从记忆中召回内容并注入到当前会话上下文时记录。

#### 2. Promotion Applied（记忆提升）

```typescript
type MemoryHostPromotionAppliedEvent = {
  type: "memory.promotion.applied";
  timestamp: number;
  sessionKey: string;
  promotionId: string;
  fromLevel: string;        // 原记忆层级
  toLevel: string;          // 提升后层级
  reason: string;           // 提升原因
};
```

当某条记忆因重要性被提升到更高层级（如从短期记忆提升到长期记忆）时记录。

#### 3. Dream Completed（Dream 完成）

```typescript
type MemoryHostDreamCompletedEvent = {
  type: "memory.dream.completed";
  timestamp: number;
  dreamId: string;
  phase: "light" | "deep" | "rem";  // Dream 阶段
  summary: string;
  memoriesProcessed: number;
  memoriesCreated: number;
};
```

当后台 Dreaming 过程完成一轮记忆整理时记录。

### 事件日志存储

```typescript
export const MEMORY_HOST_EVENT_LOG_RELATIVE_PATH = path.join(
  "memory",
  ".dreams",
  "events.jsonl",
);
```

- **存储位置**：`{workspace}/memory/.dreams/events.jsonl`
- **格式**：JSON Lines（每行一个 JSON 对象，便于追加读取）
- **函数接口**：
  - `appendMemoryHostEvent(workspaceDir, event)` — 追加事件
  - `readMemoryHostEvents(workspaceDir)` — 读取所有事件

### 事件用途

- **记忆溯源**：追踪某条记忆何时、从何地被召回
- **效果评估**：分析记忆提升策略的有效性
- **Dream 监控**：追踪后台整理任务的执行情况
- **调试诊断**：排查记忆相关的异常行为

---

## 4. Dreaming — 后台记忆整理机制

### 核心概念

Dreaming 是 OpenClaw 的自动记忆整理机制，类似于人类睡眠时的记忆巩固过程。它在后台定期运行，将零散的记忆片段组织、归纳、去重，形成更持久的长期记忆。

### 执行配置

`src/memory-host-sdk/dreaming.ts` 定义了 Dreaming 的执行参数：

```typescript
export const DEFAULT_MEMORY_DREAMING_FREQUENCY = "0 3 * * *"; // 每天凌晨 3 点

export type MemoryDreamingSpeed = "fast" | "balanced" | "slow";
export type MemoryDreamingThinking = "low" | "medium" | "high";
export type MemoryDreamingBudget = "cheap" | "medium" | "expensive";

export type MemoryDreamingConfig = {
  enabled: boolean;
  frequency: string;           // Cron 表达式
  speed: MemoryDreamingSpeed;  // 执行速度
  thinking: MemoryDreamingThinking;  // 思考深度
  budget: MemoryDreamingBudget;      // 预算等级
};
```

### 三阶段 Dream 模型

```typescript
export type MemoryDreamingPhase = "light" | "deep" | "rem";
```

#### Light Dream（浅层整理）
- **频率**：最高（可每小时执行）
- **任务**：简单的去重、标签整理、过期清理
- **成本**：低（cheap budget）
- **模型**：轻量级模型即可

#### Deep Dream（深度整理）
- **频率**：每天一次（默认凌晨 3 点）
- **任务**：主题聚类、摘要生成、关联建立
- **成本**：中等（medium budget）
- **模型**：需要较强的推理能力

#### REM Dream（ REM 整理）
- **频率**：每周或按需
- **任务**：跨会话关联、模式发现、知识图谱更新
- **成本**：高（expensive budget）
- **模型**：最强推理能力

### 去重与恢复机制

- **Deduplication**：通过向量相似度检测重复记忆，合并或删除冗余内容
- **Recovery**：如果某次 Dream 失败，下次执行时从断点恢复，避免重复处理
- **Workspace 隔离**：每个 workspace 独立进行 Dreaming，互不干扰

### Dream 执行流程

```
Cron Trigger (每天 3:00 AM)
  → 检查 workspace 是否有新记忆需要整理
    → Light Dream（快速去重和标签整理）
      → Deep Dream（主题聚类和摘要生成）
        → REM Dream（跨会话关联，可选）
          → 记录 DreamCompletedEvent 到 events.jsonl
            → 更新记忆的层级和索引
```

---

## 总结对比

| 机制 | Manta（已实现） | OpenClaw |
|------|----------------|----------|
| **多模态存储** | `multimodal.rs` — Image + Audio 双模态，10MB 限制，自动分类 | Image + Audio 双模态，10MB 限制，自动分类 |
| **语义查询** | `qmd.rs` + `hybrid.rs` — QMD CLI + 向量/FTS5 混合搜索 | QMD CLI 工具 + 向量搜索 + Scope 访问控制 |
| **事件追踪** | `events.rs` — JSONL 事件日志（recall/promotion/compact/dream） | JSONL 事件日志（recall/promotion/dream） |
| **记忆整理** | `dreaming.rs` — Light/Deep/REM 三阶段，Cron 调度，embedding 去重 | Light/Deep/REM 三阶段 Dreaming，Cron 调度 |
| **访问控制** | `QmdScope` — channel/chatType/keyPrefix/allow/deny | 基于 channel/chatType/keyPrefix 的 QMD Scope |
| **文件管理** | `MultimodalStore` — 按模态分类存储，Glob 扫描，大小限制 | 按模态分类存储，Glob 扫描，大小限制 |
| **知识图谱** | `KnowledgeGraph` + 磁盘持久化 | REM 阶段知识图谱 |

---

## 对 Manta 的借鉴建议

> 以下建议中的 ✅ 标记表示已在 Manta 中实现。

### 短期（已实现 ✅）

1. **多模态文件存储** ✅
   - `src/memory/multimodal.rs` 已支持 image（7 种格式）和 audio（7 种格式）
   - 配置项：`MemoryMultimodalConfig`（enabled, modalities, max_file_bytes）

2. **事件日志系统** ✅
   - `MemoryEvent` enum：RecallRecorded / PromotionApplied / CompactCompleted / DreamCompleted
   - JSONL 追加写入 `memory/.dreams/events.jsonl`
   - `MemoryEventLog` 提供 `append()` / `read_all()` / `read_by_type()`
   - 触发场景：recall（`retrieve()`）、promotion（dreaming）、compact（`compact_session()`）、dream（`run_full_cycle()`）

3. **QMD 集成基础** ✅
   - `QmdExecutor` 封装 `qmd` CLI，含 `is_available()` / `query()`
   - `QmdQueryResult` 结构完整，已接入 `MemoryManager::retrieve()`

### 中期（已实现 ✅）

4. **Scope-based 访问控制** ✅
   - `QmdScope` 含 channel / chat_type / key_prefix / allow / deny
   - `MemoryManager::retrieve()` 使用 `key_prefix: "{user_id}:"` 做范围隔离
   - `QmdScope::is_allowed()` 实现 deny 优先 + allow 过滤 + prefix 匹配

5. **基础 Dreaming 机制** ✅
   - `DreamEngine` + `DreamScheduler`（cron 调度，`tokio::spawn`）
   - Light Dream：embedding cosine similarity 去重（>0.95），fallback 文本 hash
   - Deep Dream：基于共享词的主题聚类 + 摘要生成
   - 事件日志：每阶段完成后写入 `DreamCompleted`

6. **多模态召回集成** ✅
   - `MultimodalStore` 按模态分类存储（`memory/images/`、`memory/audio/`）
   - `session_context()` 自动扫描 multimodal 文件并注入上下文标签
   - QMD 召回已接入（检索文本相关内容）

### 长期

7. **完整 Dreaming 体系**（部分实现）
   - ✅ REM Dream：跨 session 关联、模式发现
   - ✅ 轻量级知识图谱：`KnowledgeNode` / `KnowledgeEdge` / `KnowledgeGraph`
   - ✅ 知识图谱持久化：自动保存/加载 `memory/.dreams/knowledge_graph.json`
   - ⬜ Dream 结果的人工审核和修正
   - ⬜ 资源消耗记录与可观测性仪表盘

8. **记忆分层系统** ✅
   - `TieredStore`：Working（InMemory）→ ShortTerm（SQLite）→ LongTerm（SQLite）→ Archival（压缩 JSONL）
   - `TierEvaluator` 负责层间晋升/降级，`TierIndex` 维护内存索引
   - 每层独立 TTL、容量、最小 importance 阈值

9. **记忆效果评估**
   - ✅ `EffectivenessTracker` 追踪召回命中率（`record_recall()`）
   - ⬜ 基于使用效果自动调整权重和层级的闭环反馈

---

## 参考代码位置（OpenClaw）

| 文件 | 职责 |
|------|------|
| `src/memory-host-sdk/host/multimodal.ts` | 多模态文件分类与配置 |
| `src/memory-host-sdk/host/qmd-query-parser.ts` | QMD 查询结果解析 |
| `src/memory-host-sdk/host/qmd-scope.ts` | QMD 范围访问控制 |
| `src/memory-host-sdk/host/qmd-process.ts` | QMD CLI 执行封装 |
| `src/memory-host-sdk/events.ts` | 记忆事件类型与 JSONL 日志 |
| `src/memory-host-sdk/dreaming.ts` | Dreaming 配置与调度 |
| `src/memory-host-sdk/host/memory-host.ts` | Memory Host 核心入口 |

