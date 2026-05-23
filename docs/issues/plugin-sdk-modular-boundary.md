# Issue: Plugin SDK 的精细化模块拆分和边界控制

## 背景

Manta 当前的 plugin 系统基于 WASM 运行时（`wasmtime`），但缺乏成熟的 Plugin SDK。OpenClaw 的 Plugin SDK 是一个极其精细化的模块体系，通过 200+ 个子路径导出（`plugin-sdk/*`），实现了严格的边界控制和依赖隔离。这套机制是 OpenClaw 扩展生态能够健康增长的基础。

---

## OpenClaw 的 Plugin SDK 架构

### 1. 子路径导出的设计哲学

OpenClaw 的 `package.json` 中声明了超过 200 个 `plugin-sdk/*` 导出路径：

```json
{
  "exports": {
    "./plugin-sdk/runtime": {
      "types": "./dist/plugin-sdk/runtime.d.ts",
      "default": "./dist/plugin-sdk/runtime.js"
    },
    "./plugin-sdk/agent-runtime": {
      "types": "./dist/plugin-sdk/agent-runtime.d.ts",
      "default": "./dist/plugin-sdk/agent-runtime.js"
    },
    "./plugin-sdk/channel-runtime": {
      "types": "./dist/plugin-sdk/channel-runtime.d.ts",
      "default": "./dist/plugin-sdk/channel-runtime.js"
    },
    ...
  }
}
```

每个子路径对应一个**独立编译单元**，插件只引入自己需要的模块，不会拉入整个 SDK。

### 2. 模块分类体系

OpenClaw 的 Plugin SDK 按功能域划分为多个大类：

#### 核心运行时
- `runtime.ts` — 插件运行时环境
- `runtime-env.ts` — 环境变量访问
- `runtime-logger.ts` — 日志接口
- `runtime-config-snapshot.ts` — 配置快照
- `runtime-group-policy.ts` — 组策略
- `runtime-doctor.ts` — 诊断工具

#### Agent 相关
- `agent-runtime.ts` — Agent 运行时接口
- `agent-harness.ts` — Agent harness 抽象
- `agent-harness-runtime.ts` — Harness 运行时
- `agent-config-primitives.ts` — Agent 配置原语
- `agent-media-payload.ts` — Agent 媒体载荷

#### Provider 相关
- `provider-auth.ts` / `provider-auth-runtime.ts` — Provider 认证
- `provider-http.ts` — HTTP 传输
- `provider-stream.ts` / `provider-stream-shared.ts` — 流处理
- `provider-tools.ts` — 工具 schema
- `provider-selection-runtime.ts` — 模型选择
- `provider-transport-runtime.ts` — 传输层

#### Channel 相关
- `channel-runtime.ts` — Channel 运行时
- `channel-runtime-context.ts` — Channel 上下文
- `channel-streaming.ts` — 流式消息
- `channel-reply-pipeline.ts` — 回复流水线
- `channel-entry-contract.ts` / `channel-contract.ts` — Channel 契约

#### Memory 相关
- `memory-core.ts` — 记忆核心接口
- `memory-core-engine-runtime.ts` — 引擎运行时
- `memory-core-host-engine-foundation.ts` — Host 引擎基础
- `memory-core-host-engine-qmd.ts` — QMD 查询
- `memory-core-host-engine-storage.ts` — 存储层
- `memory-core-host-multimodal.ts` — 多模态
- `memory-core-host-events.ts` — 事件系统

#### Approval / Security
- `approval-runtime.ts` — 审批运行时
- `approval-auth-runtime.ts` — 审批认证
- `approval-handler-runtime.ts` — 审批处理
- `security-runtime.ts` — 安全运行时

#### 工具与命令
- `setup-tools.ts` — 工具注册
- `command-auth.ts` / `command-auth-native.ts` — 命令认证
- `command-gating.ts` — 命令门控
- `command-surface.ts` — 命令表面

### 3. 边界控制机制

#### 内部导出隔离
OpenClaw 严格区分 `plugin-sdk/`（对外）和 `src/`（内部）：

- `plugin-sdk/` 目录下的模块是**唯一**被允许从插件导入的代码
- `src/` 下的模块**禁止**被插件直接引用（通过自定义 lint 规则 `check-extension-plugin-sdk-boundary` 强制执行）

#### 运行时依赖注入
每个 SDK 模块通过运行时上下文获取依赖，而非静态导入：

```typescript
// plugin-sdk/runtime.ts
export function getPluginRuntime(): PluginRuntime {
  // 从全局运行时获取，而非直接引用内部模块
}
```

#### 契约测试
OpenClaw 为每个 channel 和 plugin 表面定义了契约测试：
- `test:contracts:channels` — 验证 channel 插件的 API 兼容性
- `test:contracts:plugins` — 验证插件的 API 兼容性

### 4. Plugin Entry 接口

`src/plugin-sdk/plugin-entry.ts` 定义了插件需要实现的完整接口：

```typescript
type OpenClawPluginApi = {
  // 模型目录增强
  augmentModelCatalog?: (ctx: ProviderCatalogContext) => ProviderCatalogResult;
  // 认证方法
  authMethods?: ProviderAuthMethod[];
  // Channel 实现
  channel?: ChannelPluginContract;
  // 命令定义
  commands?: OpenClawPluginCommandDefinition[];
  // HTTP 路由
  httpRoutes?: OpenClawPluginHttpRouteHandler[];
  // 工具工厂
  tools?: OpenClawPluginToolFactory[];
  // 安全审计
  securityAudit?: OpenClawPluginSecurityAuditCollector;
  // 服务注册
  services?: OpenClawPluginService[];
  // ...
};
```

### 5. 诊断与兼容性

OpenClaw 提供了完善的插件诊断：

- **Doctor 机制**：`runtime-doctor.ts` 检查插件运行时环境
- **版本兼容性**：`compat.ts` 处理不同版本间的兼容层
- **配置验证**：`config-schema.ts` 定义插件配置 schema
- **迁移系统**：`migration.ts` / `migration-runtime.ts` 支持插件数据迁移

### 6. Lint 规则保障

OpenClaw 使用大量自定义 lint 脚本来维护 SDK 边界：

```bash
lint:extensions:no-plugin-sdk-internal      # 禁止插件使用内部 API
lint:extensions:no-src-outside-plugin-sdk   # 禁止插件引用 src 目录
lint:plugins:no-extension-imports           # 禁止插件间相互导入
lint:plugins:no-monolithic-plugin-sdk-entry-imports  # 禁止整体导入 SDK
```

---

## 对 Manta 的借鉴建议

### 短期（WASM Plugin SDK 基础）

1. **定义 Plugin API Trait**
   - 在 `src/plugins/` 下定义 `PluginApi` trait，包含模型目录、工具、命令等 hook
   - 使用 WIT（WASM Interface Types）定义跨语言接口
   - 参考 OpenClaw 的 `OpenClawPluginApi`，但保持 Rust 的 trait 风格

2. **模块化 SDK 导出**
   - 为插件提供分模块的 Rust crate（如 `manta-plugin-sdk-memory`、`manta-plugin-sdk-channel`）
   - 避免插件依赖整个 `manta` crate，减少编译时间和依赖耦合
   - 使用 workspace 组织：
     ```
     crates/
     ├── manta-plugin-sdk-core/
     ├── manta-plugin-sdk-channel/
     ├── manta-plugin-sdk-memory/
     ├── manta-plugin-sdk-provider/
     └── manta-plugin-sdk-security/
     ```

3. **边界控制工具**
   - 在 CI 中添加脚本检查插件是否只使用了 SDK crate
   - 使用 `cargo-deny` 的 `bans` 规则禁止插件依赖内部 crate
   - 为每个 SDK crate 编写契约测试

### 中期（运行时能力）

4. **运行时依赖注入**
   - 插件通过 `PluginContext` 获取运行时服务，而非直接引用内部模块
   - 实现 `PluginRuntime` struct，提供配置、日志、存储等能力的受控访问
   - 使用 `Arc<dyn Trait>` 传递共享服务

5. **诊断系统**
   - 实现 `plugin-doctor` 命令，检查插件兼容性和运行时环境
   - 在加载插件时验证 WIT 版本匹配
   - 提供清晰的错误信息帮助插件开发者调试

6. **配置 Schema**
   - 定义插件配置 schema 标准（可使用 JSON Schema 或自定义 DSL）
   - 在 `Config` 中预留 `plugins: HashMap<String, PluginConfig>` 空间
   - 支持配置热重载（已有 `notify-debouncer-full` feature）

### 长期（生态建设）

7. **Plugin Registry**
   - 设计插件注册中心格式（类似 OpenClaw 的 `manifest-registry`）
   - 支持插件版本解析和依赖管理
   - 插件 manifest 声明 capability（channel、provider、tool 等）

8. **兼容层**
   - 当 Plugin API 演进时，提供向后兼容层
   - 使用版本号管理 WIT 接口兼容性
   - 支持插件的多版本共存（通过 wasmtime 的 module 隔离）

---

## 参考代码位置（OpenClaw）

| 文件 | 职责 |
|------|------|
| `package.json` (exports 字段) | Plugin SDK 子路径导出声明 |
| `src/plugin-sdk/plugin-entry.ts` | 插件入口接口定义 |
| `src/plugin-sdk/runtime.ts` | 插件运行时环境 |
| `src/plugin-sdk/agent-runtime.ts` | Agent 运行时 SDK |
| `src/plugin-sdk/channel-runtime.ts` | Channel 运行时 SDK |
| `src/plugin-sdk/memory-core.ts` | Memory 核心 SDK |
| `src/plugin-sdk/provider-auth-runtime.ts` | Provider 认证 SDK |
| `src/plugin-sdk/approval-runtime.ts` | 审批运行时 SDK |
| `src/plugins/activation-planner.ts` | 插件激活计划 |
| `src/plugins/manifest.ts` | 插件 manifest 定义 |
| `src/plugins/config-schema.ts` | 插件配置 schema |
| `scripts/check-extension-plugin-sdk-boundary.mjs` | SDK 边界 lint |
| `scripts/check-plugin-sdk-subpath-exports.mjs` | 子路径导出检查 |
