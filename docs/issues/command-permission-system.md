# Issue: OpenClaw 的命令权限体系

## 背景

Manta 当前在 `src/tools/` 中实现了约 25 个原生工具（file, shell, browser 等），工具调用相对直接，缺乏复杂的命令权限控制。OpenClaw 构建了一套极其精细的命令权限体系，涵盖命令注册、检测、授权、门控、审批等多个层面，确保在多用户、多渠道环境中安全地执行命令和工具。

---

## 1. Command Registry — 命令注册表系统

### 丰富的命令元数据

OpenClaw 的每个命令不是简单的字符串映射，而是包含完整元数据的 `ChatCommandDefinition`：

```typescript
type ChatCommandDefinition = {
  key: string;                    // 唯一标识，如 "new"
  nativeName?: string;            // 原生命令名，如 "new"
  description: string;            // 描述
  textAliases: string[];          // 文本别名，如 ["/new", "/reset"]
  acceptsArgs?: boolean;          // 是否接受参数
  args?: CommandArgDefinition[];  // 参数定义
  argsParsing?: "none" | "positional";  // 参数解析方式
  formatArgs?: (values) => string; // 参数格式化
  argsMenu?: CommandArgMenuSpec | "auto"; // 参数菜单
  scope: "text" | "native" | "both"; // 作用域
  category?: CommandCategory;     // 分类
  tier?: CommandTier;             // 渐进披露层级
};
```

### 命令分类与层级

```typescript
type CommandCategory =
  | "session"      // /new, /reset, /compact, /stop
  | "options"      // /think, /model, /fast, /verbose
  | "status"       // /status, /tasks, /whoami, /context
  | "management"   // 管理类
  | "media"        // 媒体类
  | "tools"        // 工具类
  | "docks";       // 渠道切换类

/**
 * 渐进披露层级
 * - "essential": 始终可见 (~10 个核心命令)
 * - "standard": 点击展开后可见 (~15 个)
 * - "power": 仅通过搜索或显式筛选可见 (~15 个)
 */
type CommandTier = "essential" | "standard" | "power";
```

### 参数定义系统

命令参数支持丰富的定义：

```typescript
type CommandArgDefinition = {
  name: string;
  description: string;
  type: "string" | "number" | "boolean";
  required?: boolean;
  choices?: CommandArgChoice[] | CommandArgChoicesProvider; // 静态或动态选项
  preferAutocomplete?: boolean;  // 优先自动补全
  captureRemaining?: boolean;    // 捕获剩余文本
};
```

选项可以是静态列表，也可以是动态 Provider 函数（根据 config/provider/model 上下文生成）。

### 动态命令构建

`src/auto-reply/commands-registry.data.ts` 在运行时动态构建命令列表：
- 内置命令（`buildBuiltinChatCommands()`）
- 渠道插件提供的 dock 命令（每个支持 nativeCommands 的渠道自动生成 `/dock-{channelId}`）
- 版本缓存机制（按 registry version 缓存，插件变化时自动刷新）

---

## 2. Command Detection — 命令检测系统

### 三层检测机制

`src/auto-reply/command-detection.ts` 实现了三层检测：

#### 控制命令检测（`hasControlCommand`）

```typescript
function hasControlCommand(text?: string, cfg?: OpenClawConfig): boolean {
  // 1. 去除入站元数据
  const stripped = stripInboundMetadata(trimmed);
  // 2. 规范化命令体
  const normalizedBody = normalizeCommandBody(stripped, options);
  // 3. 遍历所有注册命令的 textAliases
  // 4. 精确匹配或前缀匹配（前缀后必须是空白字符）
}
```

#### 控制命令消息检测（`isControlCommandMessage`）

在 `hasControlCommand` 基础上，额外检测终止触发器（abort trigger）：
- `/stop` 等终止命令
- 内联指令检测

#### 内联命令 Token 检测（`hasInlineCommandTokens`）

```typescript
function hasInlineCommandTokens(text?: string): boolean {
  return /(?:^|\s)[/!][a-z]/i.test(body);
}
```

**设计意图**：故意偏向 false positive（宁可误判也不要漏判），因为 `CommandAuthorized` 只门控命令执行，不影响正常聊天回复。

### 命令检测决策（`shouldComputeCommandAuthorized`）

```typescript
function shouldComputeCommandAuthorized(text?: string): boolean {
  return isControlCommandMessage(text) || hasInlineCommandTokens(text);
}
```

只有当消息被检测为可能包含命令时，才会触发后续的权限计算流程。

---

## 3. Command Auth — 命令授权系统

这是整个权限体系最复杂的部分。`src/auto-reply/command-auth.ts` 实现了多层次的授权解析。

### Provider 推断

首先需要从消息上下文中推断出所属的 channel provider：

```typescript
function resolveProviderFromContext(ctx: MsgContext, cfg: OpenClawConfig): {
  providerId: ChannelId | undefined;
  hadResolutionError: boolean;
} {
  // 1. 从显式渠道字段解析（Surface, OriginatingChannel, Provider）
  // 2. 从 From/To 的冒号分隔格式推断（如 "telegram:12345"）
  // 3. 从已加载的渠道插件中 probe 匹配
}
```

**Probe 推断**：当无法直接解析时，遍历所有已加载的渠道插件，通过 `resolveAllowFrom` 返回非空列表的插件作为候选。

### AllowFrom 解析

多来源的 AllowFrom 解析，含错误回退：

```typescript
function buildProviderAllowFromResolution(params: {
  plugin?: ChannelPlugin;
  cfg: OpenClawConfig;
  accountId?: string | null;
  providerId?: ChannelId;
  forceFallbackResolutionError?: boolean;
}): ProviderAllowFromResolution {
  // 1. 优先使用插件的 config.resolveAllowFrom()
  // 2. 出错时回退到配置文件的 fallback allowFrom
  // 3. 记录 hadResolutionError 标志
}
```

### Owner 授权状态机

```typescript
type OwnerAuthorizationState = {
  allowAll: boolean;                // 是否允许所有人
  ownerAllowAll: boolean;           // owner 配置是否允许所有人
  ownerCandidatesForCommands: string[]; // 命令的 owner 候选列表
  explicitOwners: string[];         // 显式配置的 owner
  ownerList: string[];              // 最终 owner 列表
};
```

**Owner 解析逻辑**：
1. 如果 AllowFrom 包含通配符 `*`，则 `allowAll = true`
2. 否则从 `commands.ownerAllowFrom` 解析 owner 列表
3. 支持 provider 前缀格式：`telegram:12345` 表示仅 telegram 渠道的 12345
4. 渠道插件可以通过 `config.formatAllowFrom()` 自定义格式

### Sender 候选解析

从消息上下文中解析所有可能的 sender 标识：

```typescript
function resolveSenderCandidates(params: {
  plugin, providerId, cfg, accountId,
  senderId, senderE164, from, chatType
}): string[] {
  // 返回所有可能的 sender 标识候选
  // 包括 senderId、senderE164、from 等
}
```

### 完整的命令授权解析

```typescript
function resolveCommandAuthorization(params: {
  ctx: MsgContext;
  cfg: OpenClawConfig;
  commandAuthorized: boolean;  // 来自 Command Gating 的结果
}): CommandAuthorization {
  // 1. 推断 Provider
  // 2. 解析 AllowFrom
  // 3. 解析 Owner 状态
  // 4. 解析 Sender 候选
  // 5. 匹配 Sender 是否在 Owner 列表中
  // 6. 检查 Gateway Client Scope（operator.admin 自动成为 owner）
  // 7. 计算 senderIsOwner（支持 ForceSenderIsOwnerFalse 强制覆盖）
  // 8. 计算 isOwnerForCommands（考虑 enforceOwner + ownerAllowlistConfigured）
  // 9. 计算最终的 isAuthorizedSender
}
```

**返回结果**：
```typescript
type CommandAuthorization = {
  providerId?: ChannelId;
  ownerList: string[];       // 完整的 owner 列表
  senderId?: string;         // 匹配到的 sender ID
  senderIsOwner: boolean;    // sender 是否是 owner
  isAuthorizedSender: boolean; // 最终授权结果
  from?: string;
  to?: string;
};
```

### Commands AllowFrom 配置

支持按 provider 独立配置的 AllowFrom：

```yaml
# 配置文件示例
commands:
  allowFrom:
    telegram: ["123456789", "987654321"]  # telegram 专用
    "*": ["admin@example.com"]            # 全局 fallback
  ownerAllowFrom:
    - "telegram:123456789"  # 仅 telegram 渠道的该用户是 owner
    - "admin@example.com"   # 所有渠道的该用户是 owner
```

---

## 4. Command Gating — 命令门控系统

`src/channels/command-gating.ts` 实现了基于访问组的命令门控。

### Authorizer 模型

```typescript
type CommandAuthorizer = {
  configured: boolean;  // 是否已配置
  allowed: boolean;     // 是否允许
};
```

### 访问组关闭时的三种模式

```typescript
type CommandGatingModeWhenAccessGroupsOff = "allow" | "deny" | "configured";
```

- **allow**：访问组关闭时，所有人都可以使用命令
- **deny**：访问组关闭时，所有人都不能使用命令
- **configured**：访问组关闭时，如果有任何 authorizer 配置了，则按配置决定；否则允许所有人

### 多 Authorizer 决策

```typescript
function resolveCommandAuthorizedFromAuthorizers(params: {
  useAccessGroups: boolean;
  authorizers: CommandAuthorizer[];
  modeWhenAccessGroupsOff?: CommandGatingModeWhenAccessGroupsOff;
}): boolean {
  if (!useAccessGroups) {
    // 访问组关闭时的模式处理
  }
  // 访问组开启时：任何一个已配置且允许的 authorizer 即可通过（OR 逻辑）
  return authorizers.some((entry) => entry.configured && entry.allowed);
}
```

### 控制命令门控

```typescript
function resolveControlCommandGate(params: {
  useAccessGroups: boolean;
  authorizers: CommandAuthorizer[];
  allowTextCommands: boolean;
  hasControlCommand: boolean;
}): { commandAuthorized: boolean; shouldBlock: boolean } {
  const commandAuthorized = resolveCommandAuthorizedFromAuthorizers(...);
  const shouldBlock = allowTextCommands && hasControlCommand && !commandAuthorized;
  return { commandAuthorized, shouldBlock };
}
```

当消息包含控制命令但 sender 未授权时，`shouldBlock = true`。

### 双 Authorizer 支持

```typescript
function resolveDualTextControlCommandGate(params: {
  useAccessGroups: boolean;
  primaryConfigured: boolean;
  primaryAllowed: boolean;
  secondaryConfigured: boolean;
  secondaryAllowed: boolean;
  hasControlCommand: boolean;
}): { commandAuthorized: boolean; shouldBlock: boolean };
```

支持主次两个 authorizer（如 DM 策略 + Group 策略）。

---

## 5. Command Status Builders — 命令状态展示

`src/auto-reply/command-status-builders.ts` 实现了命令帮助信息的动态构建。

### Help 消息构建

```typescript
function buildHelpMessage(cfg?: OpenClawConfig): string {
  // Session: /new | /reset | /compact [instructions] | /stop
  // Options: /think <level> | /model <id> | /fast status|on|off | ...
  // Status: /status | /tasks | /whoami | /context
  // Skills: /skill <name> [input]
}
```

支持 feature flag 控制：`isCommandFlagEnabled(cfg, "config")`、`isCommandFlagEnabled(cfg, "debug")`

### Commands 列表分页

```typescript
type CommandsMessageResult = {
  text: string;
  totalPages: number;
  currentPage: number;
  hasNext: boolean;
  hasPrev: boolean;
};
```

- 每页 8 个命令
- 按 category 分组展示
- 支持 Telegram 分页键盘

---

## 6. Tool Gating — 工具门控系统

### OpenClaw Tools 注册

`src/agents/openclaw-tools.ts` 是核心工具注册入口，`createOpenClawTools()` 函数接受 30+ 个选项参数：

```typescript
type CreateOpenClawToolsOptions = {
  sandboxBrowserBridgeUrl?: string;
  allowHostBrowserControl?: boolean;
  agentSessionKey?: string;
  agentChannel?: GatewayMessageChannel;
  sandboxRoot?: string;
  sandboxed?: boolean;
  fsPolicy?: ToolFsPolicy;
  pluginToolAllowlist?: string[];      // 插件工具白名单
  modelHasVision?: boolean;            // 模型视觉能力门控
  modelProvider?: string;              // Provider 门控
  modelId?: string;                    // 模型门控
  allowMediaInvokeCommands?: boolean;
  disableMessageTool?: boolean;
  disablePluginTools?: boolean;
  requesterSenderId?: string | null;
  senderIsOwner?: boolean;             // Owner 门控
  sessionId?: string;
  // ... 还有更多
};
```

### 工具白名单

- `pluginToolAllowlist` — 显式允许使用的插件工具列表
- `disablePluginTools` — 完全禁用所有插件工具
- `disableMessageTool` — 禁用消息发送工具

### 模型/Provider 门控

- `modelHasVision` — 只有模型支持视觉时才注册图像相关工具
- `modelProvider` / `modelId` — 特定 provider/model 的工具可用性控制
- `isUpdatePlanToolEnabledForOpenClawTools()` — 根据执行契约动态决定是否启用 plan 工具

### 沙箱与权限

- `sandboxed` — 是否运行在沙箱中（影响文件系统访问）
- `sandboxRoot` — 沙箱根目录限制
- `fsPolicy` — 文件系统访问策略
- `allowHostBrowserControl` — 是否允许控制宿主机浏览器

---

## 7. Exec Approvals — 执行审批系统

### Bash 工具的安全分析

`src/agents/bash-tools.exec.ts` 实现了 shell 命令执行的完整安全体系：

```typescript
import { analyzeShellCommand } from "../infra/exec-approvals-analysis.js";
import {
  type ExecAsk, type ExecHost, type ExecSecurity,
  loadExecApprovals, maxAsk, minSecurity,
} from "../infra/exec-approvals.js";
```

### 执行审批配置

```typescript
type ExecAsk = "never" | "dangerous" | "always";
type ExecHost = "auto" | "node" | "gateway";
type ExecSecurity = "sandbox" | "normal" | "relaxed";
```

- **Ask**：何时请求审批（从不 / 危险命令 / 总是）
- **Host**：在哪执行（自动选择 / Node.js / Gateway）
- **Security**：安全级别（沙箱 / 正常 / 宽松）

### 命令安全分析

`src/infra/exec-approvals-analysis.ts` 分析 shell 命令的风险等级：
- 识别危险操作（rm, dd, mkfs 等）
- 识别网络操作（curl, wget 等）
- 识别文件系统操作
- 根据风险等级决定是否需要审批

### 执行环境安全

```typescript
import { resolveExecSafeBinRuntimePolicy } from "../infra/exec-safe-bin-runtime-policy.js";
import { sanitizeHostExecEnvWithDiagnostics } from "../infra/host-env-security.js";
```

- **Safe Bin Policy**：限制可执行的二进制文件范围
- **Host Env Security**：清理执行环境变量，防止信息泄露
- **Shell Env Fallback**：处理 shell 环境变量加载超时

---

## 总结对比

| 机制 | Manta（当前） | OpenClaw |
|------|-------------|----------|
| **命令注册** | 无显式注册表 | 完整的 `ChatCommandDefinition` + tier/category/scope/args |
| **命令检测** | 无 | 三层检测（控制命令 / 命令消息 / 内联 token） |
| **命令授权** | 无 | Provider 推断 → AllowFrom 解析 → Owner 状态机 → Sender 匹配 |
| **命令门控** | 无 | 访问组 + 多 Authorizer + 双 Authorizer + 阻塞决策 |
| **Help 系统** | 无 | 动态构建 + feature flag + 分页 |
| **工具门控** | 简单 feature flag | 30+ 选项 + 白名单 + model/provider 门控 + owner 门控 |
| **执行审批** | 无 | Ask/Host/Security 三级 + 命令安全分析 + 环境清理 |

---

## 对 Manta 的借鉴建议

### 短期

1. **命令注册表**
   - 在 `src/cli/` 或 `src/tools/` 中实现 `CommandRegistry`，支持 `/new`, `/reset`, `/status` 等命令
   - 每个命令定义 key、aliases、description、args、category
   - 支持文本别名检测（如 `/new`, `/reset`）

2. **基础命令授权**
   - 在配置中增加 `commands.allow_from` 字段
   - 支持按 channel 配置的允许列表
   - 实现简单的 owner 检测（配置中的 owner ID 列表）

3. **命令检测**
   - 实现 `has_control_command()` 函数检测文本中的命令
   - 支持前缀匹配（`/command args`）
   - 支持内联 token 检测（`hey /status`）

### 中期

4. **命令门控**
   - 实现基于访问组的命令权限控制
   - 支持多 authorizer 的 OR 逻辑
   - 支持访问组关闭时的三种模式

5. **Help 系统**
   - 实现 `manta help` 命令
   - 动态构建帮助信息，支持 feature flag 控制显示
   - 按 category 分组展示

6. **工具门控**
   - 在 `create_tools()` 中增加选项参数：
     - `plugin_tool_allowlist` — 插件工具白名单
     - `model_has_vision` — 视觉工具门控
     - `sender_is_owner` — Owner 工具门控
   - 支持按 model/provider 的工具可用性控制

### 长期

7. **执行审批**
   - 为 shell 工具增加审批配置：`ask = "never" | "dangerous" | "always"`
   - 实现命令安全分析（识别 rm, curl 等危险命令）
   - 实现执行环境清理（`sanitize_host_exec_env`）
   - 支持审批请求的交互式处理

8. **渐进披露**
   - 为命令定义 tier（essential/standard/power）
   - 在 UI 中按 tier 分层展示
   - 支持命令搜索和筛选

---

## 参考代码位置（OpenClaw）

| 文件 | 职责 |
|------|------|
| `src/auto-reply/commands-registry.types.ts` | 命令注册表类型定义 |
| `src/auto-reply/commands-registry.data.ts` | 内置命令数据 + 动态构建 |
| `src/auto-reply/commands-registry.ts` | 命令注册表公共 API |
| `src/auto-reply/commands-registry-list.ts` | 命令列表查询 |
| `src/auto-reply/commands-registry-normalize.ts` | 命令规范化 |
| `src/auto-reply/command-detection.ts` | 命令检测三层机制 |
| `src/auto-reply/command-auth.ts` | 命令授权解析（最复杂） |
| `src/channels/command-gating.ts` | 命令门控决策 |
| `src/auto-reply/command-status-builders.ts` | Help/Commands 消息构建 |
| `src/agents/openclaw-tools.ts` | 工具注册入口（30+ 选项） |
| `src/agents/openclaw-tools.registration.ts` | 工具注册辅助 |
| `src/agents/bash-tools.ts` | Bash 工具导出 |
| `src/agents/bash-tools.exec.ts` | Shell 执行实现 |
| `src/infra/exec-approvals.ts` | 执行审批配置 |
| `src/infra/exec-approvals-analysis.ts` | 命令安全分析 |
| `src/infra/exec-safe-bin-runtime-policy.ts` | Safe Bin 策略 |
| `src/infra/host-env-security.ts` | 执行环境安全 |
| `src/plugin-sdk/command-auth.ts` | Plugin SDK 命令认证导出 |
| `src/plugin-sdk/command-gating.ts` | Plugin SDK 命令门控导出 |
