# Issue: Extension 热加载和版本管理

## 背景

Manta 当前使用 WASM 插件系统（`wasmtime` + `wit-bindgen`），支持编译时加载插件。OpenClaw 在此基础上实现了更成熟的**运行时热加载**和**版本管理**机制，允许在不停机的情况下安装、更新、卸载扩展。这套机制对于长期运行的 AI Gateway 服务至关重要。

---

## OpenClaw 的 Extension 架构

### 1. 多层级扩展体系

OpenClaw 的扩展分为三个层级：

#### 内置插件（Bundled Plugins）
随 OpenClaw 核心一起发布的插件，如：
- `browser` — 浏览器自动化
- `active-memory` — 主动记忆
- `acpx` — ACP 扩展协议

这些插件的代码位于 `extensions/` 目录，有自己的 `package.json`：

```json
{
  "name": "@openclaw/browser-plugin",
  "version": "2026.4.25",
  "openclaw": {
    "extensions": ["./index.ts"]
  }
}
```

#### 外部插件（External Plugins）
通过 npm/pnpm 安装的第三方插件：
- 支持 `workspace:*` 和 semver 版本解析
- 安装后通过 `postinstall` 脚本注册到运行时

#### 用户扩展（User Extensions）
用户自行开发的扩展，通过配置引用：
- 支持本地路径引用
- 支持 Git URL 引用

### 2. 插件激活计划（Activation Planner）

`src/plugins/activation-planner.ts` 是 OpenClaw 插件系统的核心调度器：

```typescript
type PluginActivationPlannerTrigger =
  | { kind: "command"; command: string }
  | { kind: "provider"; provider: string }
  | { kind: "agentHarness"; runtime: string }
  | { kind: "channel"; channel: string }
  | { kind: "route"; route: string }
  | { kind: "capability"; capability: PluginManifestActivationCapability };

type PluginActivationPlan = {
  trigger: PluginActivationPlannerTrigger;
  pluginIds: readonly string[];
  entries: readonly PluginActivationPlanEntry[];
  diagnostics: readonly PluginDiagnostic[];
};
```

**激活逻辑**：
1. 根据 trigger（命令/提供商/渠道等）确定需要加载哪些插件
2. 从 manifest registry 中查询匹配的插件
3. 生成激活计划，包含诊断信息
4. 按依赖顺序加载插件

### 3. Manifest Registry

每个插件必须提供 manifest，声明其能力：

```typescript
type PluginManifestActivationCapability =
  | "channel"
  | "provider"
  | "tool"
  | "command"
  | "route"
  | "migration"
  | "securityAudit"
  | "service";
```

`src/plugins/manifest-registry.ts` 维护全局的插件注册表：
- 扫描 `extensions/` 目录和已安装包
- 解析每个插件的 manifest
- 缓存解析结果，支持运行时刷新

### 4. 运行时注册表（Active Runtime Registry）

`src/plugins/active-runtime-registry.ts` 管理**已激活**的插件运行时：

```typescript
export function getActiveRuntimePluginRegistry(): PluginRegistry | null {
  return loadPluginRuntime()?.getActivePluginRegistry() ?? null;
}
```

- 懒加载运行时模块（`runtime.js` / `runtime.ts`）
- 维护激活状态的插件列表
- 支持运行时查询活跃插件

### 5. 版本管理

OpenClaw 使用基于日期的版本号（`2026.4.26`），包含以下版本管理策略：

#### 插件版本同步
```bash
plugins:sync  # 同步插件版本到统一版本号
```

#### 运行时依赖管理
每个内置插件有自己的 `dependencies` 和 `devDependencies`：
- 运行时依赖在插件激活时解析
- 通过 `stage-bundled-plugin-runtime-deps` 脚本预打包运行时依赖

#### 兼容性检查
- `release:plugins:clawhub:check` — 检查插件与核心版本的兼容性
- `release:plugins:npm:check` — 检查 npm 发布状态
- `test:build:bundled-runtime-deps` — 验证打包后的运行时依赖完整性

### 6. 热加载机制

#### 配置重载触发
OpenClaw 支持通过 `config-reload` 触发插件重新加载：
- 配置文件变更时检测插件配置变化
- 只加载/卸载发生变化的插件
- 保持未变更插件的运行时状态

#### 运行时生命周期
```
Config Change Detected
  → Plugin Activation Planner 重新计算
    → 对比当前激活状态
      → 卸载移除的插件
      → 加载新增的插件
      → 更新配置变更的插件
        → 更新 Active Runtime Registry
```

### 7. 安装/卸载流程

#### 安装流程（test:docker:bundled-plugin-install-uninstall）
```bash
1. 下载插件包
2. 解析 manifest
3. 安装运行时依赖（npm install）
4. 注册到 manifest registry
5. 执行插件 setup（如果有）
6. 激活插件 capability
```

#### 卸载流程
```bash
1. 停用插件 capability
2. 从 active runtime registry 移除
3. 执行 cleanup hooks
4. 从 manifest registry 移除
5. 可选：删除运行时依赖
```

### 8. 安全边界

OpenClaw 对插件运行时实施严格的安全控制：

- **Sandbox 隔离**：浏览器插件使用独立的 sandbox 容器（`Dockerfile.sandbox-browser`）
- **权限最小化**：插件只能访问其 manifest 声明的 capability
- **命令门控**：`command-gating.ts` 控制插件命令的执行权限
- **安全审计**：`securityAudit` capability 允许插件参与安全审计

---

## 对 Manta 的借鉴建议

### 短期（WASM 热重载基础）

1. **运行时插件加载**
   - 使用 `wasmtime` 的 `Instance` 重新创建能力实现热加载
   - 在 `src/plugins/runtime.rs` 中实现 `PluginRuntime::reload(plugin_id)`
   - 通过文件系统监听（`notify` crate）检测 `.wasm` 文件变更

2. **插件注册表**
   - 实现 `PluginRegistry` struct，维护已安装插件的元数据
   - 使用 SQLite（`src/memory/db.rs`）持久化插件注册信息
   - 设计 TOML 格式的插件 manifest：
     ```toml
     [plugin]
     id = "browser"
     name = "Browser Automation"
     version = "0.1.0"
     capabilities = ["tool", "browser"]

     [plugin.wasm]
     path = "plugins/browser.wasm"
     ```

3. **激活计划简化版**
   - 在启动时根据 `Config.plugins` 生成激活计划
   - 按 capability 分组加载（先 provider，再 channel，最后 tool）
   - 记录加载诊断信息到日志

### 中期（版本管理）

4. **插件版本解析**
   - 支持语义化版本（`semver` crate）
   - 在 manifest 中声明兼容性要求：
     ```toml
     [plugin.compatibility]
     manta = ">=0.1.0, <0.2.0"
     ```
   - 加载前验证版本兼容性，不兼容时拒绝加载并给出清晰错误

5. **配置热重载集成**
   - 当 `ConfigWatcher` 检测到配置变更时：
     1. 解析新的插件列表
     2. 对比当前已加载的插件
     3. 计算差异（新增、移除、配置变更）
     4. 安全地执行加载/卸载
   - 注意 WASM 实例的生命周期管理（确保旧实例完全释放）

6. **依赖管理**
   - 如果插件需要外部资源（如浏览器二进制、模型文件），在 manifest 中声明
   - 实现 `manta plugin install <id>` 命令自动下载依赖
   - 使用 `dirs` crate 管理插件数据目录

### 长期（生态与安全）

7. **插件商店/注册中心**
   - 设计插件索引格式（JSON/TOML）
   - 支持从远程 URL 安装插件：
     ```bash
     manta plugin install https://example.com/plugins/browser.toml
     ```
   - 插件签名验证（使用 `ed25519-dalek` 或类似 crate）

8. **运行时安全沙箱**
   - 为 WASM 插件配置 WASI 能力限制（文件系统访问、网络、环境变量）
   - 使用 `wasmtime_wasi::WasiCtxBuilder` 精细控制能力：
     ```rust
     let wasi = WasiCtxBuilder::new()
         .preopened_dir(plugin_dir, "/data")?
         .env("PLUGIN_ID", plugin_id)
         .build();
     ```
   - 实现插件资源限制（内存、CPU 时间）

9. **迁移系统**
   - 当插件数据结构变更时，支持数据迁移
   - 在 manifest 中声明迁移脚本路径
   - 使用 SQLite 的 schema_version 机制追踪迁移状态

---

## 参考代码位置（OpenClaw）

| 文件/目录 | 职责 |
|-----------|------|
| `extensions/` | 内置插件源码目录 |
| `extensions/browser/package.json` | 插件 manifest 示例 |
| `src/plugins/activation-planner.ts` | 插件激活计划生成 |
| `src/plugins/manifest-registry.ts` | 插件注册表 |
| `src/plugins/active-runtime-registry.ts` | 活跃运行时注册表 |
| `src/plugins/manifest.ts` | Manifest 类型定义 |
| `src/plugins/config-schema.ts` | 插件配置 schema |
| `src/plugin-sdk/plugin-runtime.ts` | 插件运行时 SDK |
| `scripts/stage-bundled-plugin-runtime-deps.mjs` | 运行时依赖打包 |
| `scripts/check-extension-package-tsc-boundary.mjs` | 扩展包边界检查 |
| `test:docker:bundled-plugin-install-uninstall` | 安装/卸载 E2E 测试 |
| `Dockerfile.sandbox-browser` | 浏览器插件 sandbox |
