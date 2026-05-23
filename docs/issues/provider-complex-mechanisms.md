# Issue: OpenClaw Provider 层的复杂机制

## 背景

Manta 当前在 `src/providers/` 中实现了对 OpenAI 和 Anthropic 的基础支持，采用简单的 trait-based 抽象。每个 provider 独立处理请求，缺乏认证档案管理、流式处理家族、用量追踪和智能路由等高级能力。OpenClaw 的 Provider 层展示了如何将 LLM 提供商支持提升为平台级能力。

---

## 1. Provider Auth — 多凭证认证档案系统

### 多凭证类型支持

OpenClaw 支持三种凭证类型，每种有独立的状态管理：

```typescript
type AuthProfileCredential =
  | ApiKeyCredential    // { type: "api_key", key?: string, keyRef?: SecretRef }
  | TokenCredential     // { type: "token", token?: string, expires?: number }
  | OAuthCredential;    // { type: "oauth", access, refresh, expires, ... }
```

### Auth Profile Store

每个 Provider 可以有**多个认证档案**（profile），存储在本地加密仓库中：

- `src/agents/auth-profiles/store.ts` — Profile Store 的持久化（支持文件锁、外部 CLI 同步）
- `src/agents/auth-profiles/profiles.ts` — Profile 的增删改查（含并发安全更新）
- `src/agents/auth-profiles/order.ts` — 按 Provider 配置 Profile 的使用优先级

### OAuth 完整认证流程

`src/plugin-sdk/provider-auth-runtime.ts` 实现了完整的 OAuth 2.0 + PKCE：

- `generatePkceVerifierChallenge()` — PKCE 挑战生成
- `waitForLocalOAuthCallback()` — 启动本地 HTTP 服务器（localhost）接收 OAuth 回调
- Token 过期自动刷新（`DEFAULT_OAUTH_REFRESH_MARGIN_MS = 5min`）
- 支持 `hasUsableOAuthCredential()` 检查凭证可用性

### 凭证状态机

`src/agents/auth-profiles/credential-state.ts` 定义了精细的凭证状态：

```typescript
type TokenExpiryState = "missing" | "valid" | "expiring" | "expired" | "invalid_expires";

type AuthCredentialReasonCode =
  | "ok" | "missing_credential" | "invalid_expires"
  | "expired" | "unresolved_ref";
```

### 失败追踪与冷却机制

每个 Profile 记录了详细的失败历史和自动冷却逻辑：

```typescript
type ProfileUsageStats = {
  lastUsed?: number;
  cooldownUntil?: number;           // 冷却截止时间
  cooldownReason?: AuthProfileFailureReason;  // 失败原因
  cooldownModel?: string;           // 触发冷却的模型
  disabledUntil?: number;           // 禁用截止时间
  disabledReason?: AuthProfileFailureReason;
  errorCount?: number;
  failureCounts?: Partial<Record<AuthProfileFailureReason, number>>;
  lastFailureAt?: number;
};
```

失败原因分类极其细致：`auth`（临时认证失败）、`auth_permanent`（永久认证失败）、`rate_limit`（速率限制）、`billing`（欠费）、`overloaded`（服务过载）、`model_not_found`、`timeout` 等。

### API Key 自动轮换

`src/agents/api-key-rotation.ts` 实现了执行时的 API Key 轮询：
- `executeWithApiKeyRotation()` — 当一个 key 失败时自动切换到下一个可用 key
- `collectProviderApiKeysForExecution()` — 收集所有可用 key 用于轮换

### Doctor 诊断系统

`src/agents/auth-profiles/doctor.ts` 提供认证问题的智能诊断：
- 检测过时的 Provider 并给出迁移指引
- 调用 Plugin 扩展的诊断 hint
- 区分"凭证缺失"和"凭证失效"给出不同建议

---

## 2. Stream Family — 流式处理家族

### 可组合的 Stream Wrapper 模式

OpenClaw 的核心抽象是 `ProviderStreamWrapperFactory` — 一个函数式包装器，可以链式组合：

```typescript
type ProviderStreamWrapperFactory =
  | ((streamFn: StreamFn | undefined) => StreamFn | undefined)
  | null | undefined | false;

function composeProviderStreamWrappers(
  baseStreamFn: StreamFn | undefined,
  ...wrappers: ProviderStreamWrapperFactory[]
): StreamFn | undefined {
  return wrappers.reduce(
    (streamFn, wrapper) => (wrapper ? wrapper(streamFn) : streamFn),
    baseStreamFn,
  );
}
```

这类似于 Rust 的 `tower::Service` 层叠模式。

### Stream Family 分类

`src/plugin-sdk/provider-stream.ts` 定义了 **Stream Family** 概念，每个 Provider 属于一个"家族"，共享特定的流式处理逻辑：

```typescript
type ProviderStreamFamily =
  | "google-thinking"           // Google Gemini 的思考模式
  | "kilocode-thinking"         // Kilocode 的 reasoning
  | "moonshot-thinking"         // Moonshot 的思考流
  | "minimax-fast-mode"         // Minimax 快速模式
  | "openai-responses-defaults" // OpenAI 默认响应
  | "openrouter-thinking"       // OpenRouter 推理
  | "tool-stream-default-on";   // 工具流默认开启
```

每个 Family 对应一组特定的 payload 包装器：

- **OpenAI Responses Defaults**：叠加 Attribution Headers → Fast Mode → Service Tier → Text Verbosity → Thinking Level → String Content → Reasoning Compatibility → Context Management
- **Google Thinking**：特殊处理 Google 的 thinking payload
- **Moonshot Thinking**：处理 thinking type 和 keep 策略
- **Minimax Fast Mode**：条件性启用 fast mode

### 特殊 Payload 处理

`src/plugin-sdk/provider-stream-shared.ts` 实现了多种通用的流式包装器：

- `createPayloadPatchStreamWrapper()` — 动态修改请求 payload
- `createHtmlEntityToolCallArgumentDecodingWrapper()` — 解码 HTML 实体编码的工具调用参数
- `applyAnthropicPayloadPolicyToParams()` — Anthropic 特有的 cache control marker
- `createToolStreamWrapper()` — 工具调用流的特殊处理

### 思考/推理模式支持

不同 Provider 的 reasoning/thinking 实现差异很大，OpenClaw 为每个做了专门适配：
- Claude 的 `thinking` 参数和 `ephemeral` cache control
- OpenAI 的 `reasoning_effort` 和 `service_tier`
- Gemini 的 `thinkingConfig`
- Moonshot 的 `thinking` type 和 keep 策略

---

## 3. Usage Tracking — 用量追踪系统

### 多 Provider 用量快照

`src/infra/provider-usage.types.ts` 定义了统一的用量数据结构：

```typescript
type UsageProviderId =
  | "anthropic" | "github-copilot" | "google-gemini-cli"
  | "minimax" | "openai-codex" | "xiaomi" | "zai";

type ProviderUsageSnapshot = {
  provider: UsageProviderId;
  displayName: string;
  windows: UsageWindow[];
  plan?: string;
  error?: string;
};

type UsageWindow = {
  label: string;        // 如 "today", "this_month"
  usedPercent: number;  // 已用百分比
  resetAt?: number;     // 重置时间戳
};
```

### 插件化用量获取

`src/infra/provider-usage.load.ts` 支持通过 Plugin 扩展获取用量：
- 优先调用 Provider Plugin 的 `resolveProviderUsageSnapshotWithPlugin()`
- 回退到内置的 fetch 实现（`provider-usage.fetch.ts`）
- 每个 Provider 有独立的 fetch 实现：`fetchClaudeUsage`、`fetchCodexUsage`、`fetchGeminiUsage` 等

### 用量认证解析

`src/infra/provider-usage.auth.ts` 实现了用量查询的认证解析：
- 从 Auth Profile Store 解析 token
- 支持多 Provider ID 别名映射
- 回退到环境变量和配置文件
- 区分"有凭证源"和"无凭证源"的 Provider

### 格式化展示

`src/infra/provider-usage.format.ts` 提供了用户友好的用量显示：
- `formatUsageSummaryLine()` — 单行摘要：`📊 Usage: Claude 87% left · Gemini 45% left`
- `formatUsageWindowSummary()` — 窗口详情：`today 87% left ⏱2h 15m`
- 自动计算重置倒计时：`2h 15m` / `3d` / `May 24`
- 智能截断：只显示用量最高的窗口

---

## 4. Model Catalog + Gateway 路由

### 统一模型目录

`src/agents/model-catalog.ts` 维护全局模型目录：

```typescript
type ModelCatalogEntry = {
  id: string;
  name: string;
  provider: string;
  alias?: string;
  contextWindow?: number;
  reasoning?: boolean;
  input?: ("text" | "image" | "audio" | "video" | "document")[];
};
```

构建来源：
1. **静态配置**：`models.json` 文件
2. **Plugin 动态发现**：`augmentModelCatalogWithProviderPlugins()`
3. **Pi SDK 延迟加载**：`pi-model-discovery-runtime.js`

### 模型抑制

`src/agents/model-suppression.runtime.js` 支持运行时屏蔽不可用模型：
- 当某个 Provider 认证失效时，自动抑制其模型
- 支持手动配置 suppression list
- 避免向已知不可用的模型发送请求

### Gateway 层模型路由

`src/gateway/models-http.ts` 暴露 OpenAI-compatible `/v1/models` 端点：
- 将内部 Agent ID 映射为模型 ID：`openclaw/{agentId}`
- 基于 Operator Scope 的权限控制
- 集成速率限制

### 请求级模型覆盖

`src/gateway/http-utils.ts` 支持通过 HTTP 参数覆盖目标模型：
- 请求头或查询参数指定模型
- 模型别名解析
- Agent 到模型的映射解析

### Provider 选择运行时

`src/plugin-sdk/provider-selection-runtime.ts` 实现了模型选择的运行时逻辑：
- 根据请求特性（vision、tools、reasoning）匹配合适模型
- 支持 fallback chain：主模型不可用时的自动降级
- 考虑 provider 的用量窗口和冷却状态

---

## 总结对比

| 机制 | Manta（当前） | OpenClaw |
|------|-------------|----------|
| **认证** | 单 API Key 配置 | 多 Profile + OAuth + Token + API Key Rotation + 冷却机制 |
| **流式** | 单一 stream 转发 | Stream Family + 可组合 Wrapper + 各 Provider 特殊处理 |
| **用量** | 无 | 7+ Provider 用量追踪 + 窗口计算 + 格式化显示 |
| **模型目录** | 硬编码 trait 实现 | 动态发现 + Plugin 扩展 + 模型抑制 + OpenAI-compatible 接口 |
| **失败处理** | 简单重试 | 失败分类 + Profile 冷却 + Key 轮换 + Doctor 诊断 |

这些机制共同构成了 OpenClaw 能够同时管理数十个 Provider、数百个模型，并在生产环境中稳定运行的基础。

---

## 对 Manta 的借鉴建议

### 短期

1. **引入 Auth Profile Store**
   - 在 SQLite 中实现多 profile 存储，支持 API Key / Token / OAuth 三种凭证
   - 实现 Profile 的优先级排序和自动轮换
   - 为每个 profile 记录失败历史，实现简单冷却机制

2. **Stream Wrapper 模式**
   - 将 `CompletionRequest` 的流处理抽象为可组合的 `StreamWrapper` trait
   - 为不同 Provider 的特殊处理（thinking、fast mode 等）实现独立 wrapper
   - 使用 `tower::Service` 风格的层叠模式组合 wrapper

3. **基础用量追踪**
   - 在 SQLite 中记录每个 provider 的请求次数和 token 消耗
   - 实现简单的用量窗口统计（今日、本月）
   - 支持阈值告警

### 中期

4. **模型目录系统**
   - 实现 `ModelCatalog` 注册表，支持静态配置和动态发现
   - 暴露 OpenAI-compatible `/v1/models` 端点
   - 实现模型抑制机制

5. **OAuth 认证**
   - 实现 PKCE 流程和本地回调服务器
   - 支持 token 自动刷新
   - 使用 `keyring` crate 安全存储凭证

6. **Doctor 诊断**
   - 实现 `manta doctor` 子命令
   - 检测认证问题并给出修复建议
   - 支持 plugin 扩展诊断逻辑

### 长期

7. **Plugin-extensible Provider**
   - 允许 WASM 插件注册新的 Provider
   - 插件声明支持的 Stream Family
   - 插件提供用量获取实现

8. **智能路由**
   - 根据请求特性（vision、tool use、reasoning）自动选择模型
   - 实现 fallback chain 和负载均衡
   - 考虑用量窗口和 provider 健康状态

---

## 参考代码位置（OpenClaw）

| 文件 | 职责 |
|------|------|
| `src/agents/auth-profiles/store.ts` | Profile Store 持久化 |
| `src/agents/auth-profiles/profiles.ts` | Profile CRUD |
| `src/agents/auth-profiles/credential-state.ts` | 凭证状态机 |
| `src/agents/auth-profiles/doctor.ts` | 认证诊断 |
| `src/agents/api-key-rotation.ts` | API Key 轮换 |
| `src/plugin-sdk/provider-auth.ts` | Provider 认证 SDK |
| `src/plugin-sdk/provider-auth-runtime.ts` | OAuth 运行时 |
| `src/plugin-sdk/provider-stream.ts` | Stream Family 定义 |
| `src/plugin-sdk/provider-stream-shared.ts` | Stream Wrapper 通用实现 |
| `src/infra/provider-usage.types.ts` | 用量类型定义 |
| `src/infra/provider-usage.load.ts` | 用量加载（含插件回退） |
| `src/infra/provider-usage.fetch.ts` | 各 Provider 用量获取 |
| `src/infra/provider-usage.format.ts` | 用量格式化显示 |
| `src/agents/model-catalog.ts` | 模型目录加载 |
| `src/agents/model-catalog.types.ts` | 模型目录类型 |
| `src/agents/model-suppression.runtime.js` | 模型抑制 |
| `src/gateway/models-http.ts` | OpenAI-compatible 模型接口 |
| `src/plugin-sdk/provider-selection-runtime.ts` | Provider 选择运行时 |
