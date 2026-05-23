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

| 机制 | Manta（当前） | OpenClaw |
|------|-------------|----------|
| **多模态存储** | 无 | Image + Audio 双模态，10MB 限制，自动分类 |
| **语义查询** | SQLite + 可选 pgvector | QMD CLI 工具 + 向量搜索 + Scope 访问控制 |
| **事件追踪** | 无 | JSONL 事件日志（recall/promotion/dream） |
| **记忆整理** | 无 | Light/Deep/REM 三阶段 Dreaming，Cron 调度 |
| **访问控制** | 无 | 基于 channel/chatType/keyPrefix 的 QMD Scope |
| **文件管理** | 无 | 按模态分类存储，Glob 扫描，大小限制 |

---

## 对 Manta 的借鉴建议

### 短期

1. **多模态文件存储**
   - 在 `src/memory/` 中增加 `multimodal.rs` 模块
   - 支持 image（png/jpg/webp）和 audio（mp3/wav/ogg）的存储和分类
   - 在 SQLite 中记录文件元数据（路径、模态、大小、创建时间）
   - 配置文件支持：
     ```toml
     [memory.multimodal]
     enabled = true
     modalities = ["image", "audio"]
     max_file_bytes = 10_485_760
     ```

2. **事件日志系统**
   - 实现 `MemoryEvent` enum（Recall / Promotion / Compact）
   - 使用 JSONL 格式追加写入 `~/.manta/memory/events.jsonl`
   - 提供 `append_memory_event()` 和 `read_memory_events()` 接口
   - 在以下场景触发事件：
     - 从记忆中召回内容注入上下文
     - 记忆因重要性被提升/降低
     - Session 被 compact/summarize

3. **QMD 集成基础**
   - 封装 `qmd` CLI 调用（如果系统已安装）
   - 实现 `QmdQueryResult` 结构和结果解析
   - 在 workspace 级别维护 `qmd` 索引

### 中期

4. **Scope-based 访问控制**
   - 为 QMD 查询增加 `QmdScope` 配置：
     ```rust
     pub struct QmdScope {
         pub channel: Option<String>,
         pub chat_type: Option<String>,
         pub key_prefix: Option<String>,
         pub allow: Vec<String>,
         pub deny: Vec<String>,
     }
     ```
   - 在查询前验证 sender 是否有权限访问目标记忆范围
   - 支持按 channel 隔离不同来源的记忆

5. **基础 Dreaming 机制**
   - 实现 `DreamEngine` struct，支持配置 Cron 调度
   - Light Dream 阶段：去重（基于 embedding 相似度 > 0.95）、过期清理
   - Deep Dream 阶段：主题聚类（k-means 或层次聚类）、自动生成摘要
   - 使用 `tokio-cron-scheduler` 实现定时触发
   - 记录 Dream 执行结果到事件日志

6. **多模态召回集成**
   - 当用户发送图片/音频时，存储到 multimodal 目录
   - 在会话上下文中引用多模态文件（如 `[Image file: screenshot.png]`）
   - 支持通过 QMD 查询找到相关的历史图片/音频

### 长期

7. **完整 Dreaming 体系**
   - 实现 REM Dream：跨 session 关联、长期模式发现
   - 构建轻量级知识图谱（实体-关系-实体）
   - 支持 Dream 结果的人工审核和修正
   - Dream 执行的可观测性（进度追踪、资源消耗记录）

8. **记忆分层系统**
   - 显式定义记忆层级：working → short-term → long-term → archival
   - 每层级有不同的存储策略（SQLite / 压缩文件 / 冷存储）
   - Dreaming 负责层间晋升和降级
   - 配置各层级的容量上限和保留策略

9. **记忆效果评估**
   - 追踪记忆召回的"命中率"（召回内容是否被 LLM 实际使用）
   - 分析哪些类型的记忆最容易被召回
   - 基于使用效果自动调整记忆的权重和层级

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

