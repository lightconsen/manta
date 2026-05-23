# Issue: Gateway 层的多模型路由和 Fallback 机制

## 背景

Manta 当前在 `src/providers/` 模块中实现了对 OpenAI 和 Anthropic 的基础支持，采用简单的 trait-based 抽象。每个 provider 独立处理请求，缺乏统一的 Gateway 层来做模型发现、负载均衡和故障转移。OpenClaw 的 Gateway 架构展示了如何将多模型支持提升为平台级能力。

---

## OpenClaw 的实现方式

### 1. 模型目录（Model Catalog）

OpenClaw 的 `src/agents/model-catalog.ts` 维护了一个**统一的模型目录**，通过以下方式构建：

- **配置文件驱动**：`ensureOpenClawModelsJson()` 确保存在一个 `models.json` 配置文件，定义可用模型及其元数据
- **Plugin 扩展发现**：`augmentModelCatalogWithProviderPlugins()` 允许 provider plugin 在运行时动态注册新模型
- **缓存策略**：模型目录在进程生命周期内缓存（`modelCatalogPromise`），支持测试时重置
- **懒加载发现**：通过动态导入 `pi-model-discovery-runtime.js` 实现 Pi SDK 模型的延迟加载

```typescript
type ModelCatalogEntry = {
  id: string;
  name?: string;
  provider: string;
  contextWindow?: number;
  reasoning?: boolean;
  input?: ModelInputType[];
};
```

### 2. Gateway HTTP 接口

`src/gateway/models-http.ts` 暴露 OpenAI-compatible `/v1/models` 端点：

- 将内部 Agent ID 映射为模型 ID（格式：`openclaw/{agentId}`）
- 支持基于 Operator Scope 的权限控制（`authorizeOperatorScopesForMethod`）
- 集成速率限制（`AuthRateLimiter`）

### 3. 多模型选择与 Fallback

OpenClaw 在 Gateway 调用链中实现了多层次的模型解析：

- **Agent 级别默认模型**：`resolveDefaultAgentId()` 确定默认 agent，每个 agent 可绑定不同模型
- **请求级别覆盖**：`http-utils.model-override` 支持通过 HTTP 头或参数覆盖目标模型
- **Provider 认证解析**：`resolveConfigApiKeyContext` 处理不同 provider 的认证方式差异
- **模型抑制（Model Suppression）**：`model-suppression.runtime.js` 允许运行时屏蔽不可用模型

### 4. 调用链架构

```
Client Request
  → Gateway HTTP Handler (models-http.ts)
    → Auth & Rate Limit (auth-rate-limit.ts)
      → Model Catalog Lookup (model-catalog.ts)
        → Agent Resolution (agent-scope.ts)
          → Provider Plugin Selection (provider-runtime-model.types.ts)
            → Backend Provider Call
              ← Response Streaming
```

### 5. 关键设计模式

- **依赖注入的 Gateway Client**：`call.ts` 中的 `defaultGatewayCallDeps` 允许在测试中注入 mock
- **凭证优先级系统**：`credentials.ts` 定义了 token > password > env > config 的优先级链
- **最小权限 Operator Scope**：`method-scopes.ts` 为每个 Gateway 方法定义所需的权限范围

---

## 对 Manta 的借鉴建议

### 短期（可行改进）

1. **引入 `ModelCatalog` 抽象**
   - 在 `src/providers/` 下新增 `model_catalog.rs`，维护统一的模型元数据注册表
   - 每个 provider 实现 `ModelCatalogSource` trait，在启动时注册支持的模型
   - 缓存模型列表，避免每次请求都查询外部 API

2. **Gateway 层的模型解析**
   - 在 `src/server/` 或新增 `src/gateway/` 中暴露 `/v1/models` 端点
   - 将内部 provider/model 组合映射为标准格式（如 OpenAI-compatible）
   - 支持通过配置指定默认模型和可用模型白名单

3. **简单的 Fallback 机制**
   - 在 `CompletionRequest` 中增加 `fallback_models: Vec<String>` 字段
   - 当主模型返回 5xx 或超时（可配置）时，按顺序尝试 fallback 模型
   - 在 `src/providers/mod.rs` 中统一处理 fallback 逻辑

### 中期（架构演进）

4. **Provider-agnostic 的 Gateway Client**
   - 抽象出 `GatewayClient` trait，封装 HTTP 连接、认证、重试逻辑
   - 支持 TLS fingerprint 验证（OpenClaw 的 `loadGatewayTlsRuntime`）
   - 实现基于 token bucket 的速率限制

5. **模型能力声明**
   - 每个 `ModelCatalogEntry` 声明能力：`context_window`, `supports_vision`, `supports_tools`, `reasoning`
   - 在路由时根据请求特性（是否需要 vision/tool/reasoning）自动匹配合适模型
   - 这是实现智能 fallback 的基础

### 长期（平台级能力）

6. **Plugin-extensible 模型发现**
   - 当 Manta 的 WASM plugin 系统成熟后，允许插件在运行时注册新 provider 和模型
   - 类似 OpenClaw 的 `augmentModelCatalogWithProviderPlugins`
   - 需要 plugin manifest 中声明 `provider` capability

---

## 参考代码位置（OpenClaw）

| 文件 | 职责 |
|------|------|
| `src/agents/model-catalog.ts` | 模型目录加载、缓存、Pi SDK 发现 |
| `src/gateway/models-http.ts` | OpenAI-compatible `/v1/models` HTTP 处理 |
| `src/gateway/call.ts` | Gateway Client 构建和调用入口 |
| `src/gateway/http-utils.ts` | 模型覆盖解析、agent 映射、认证工具 |
| `src/gateway/auth-rate-limit.ts` | Gateway 速率限制 |
| `src/gateway/method-scopes.ts` | Operator 权限范围定义 |
| `src/agents/model-catalog.types.ts` | 模型目录类型定义 |
| `src/plugins/provider-runtime-model.types.ts` | Provider 运行时模型类型 |
