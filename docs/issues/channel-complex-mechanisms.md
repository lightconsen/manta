# Issue: OpenClaw Channels 层的复杂机制

## 背景

Manta 当前在 `src/channels/` 中实现了 Telegram、Discord、Slack 等主流通信渠道，采用 feature-gated 编译。每个 channel 是相对独立的模块，处理连接、消息收发和基本的生命周期。OpenClaw 的 Channels 层展示了如何将通信渠道支持提升为平台级能力，每个 channel 都有完整的生命周期、策略、契约和插件化架构。

---

## 1. Channel Plugin 架构 — 完整的 Adapter 契约

### 20+ 个 Adapter 接口

OpenClaw 的每个 channel 是一个完整的插件，实现了 `ChannelPlugin` 类型，包含 20+ 个可选的 adapter 接口：

```typescript
type ChannelPlugin = {
  id: ChannelId;
  meta: ChannelMeta;
  capabilities: ChannelCapabilities;
  defaults?: { queue?: { debounceMs?: number } };
  reload?: { configPrefixes: string[]; noopPrefixes?: string[] };
  setupWizard?: ChannelPluginSetupWizard;
  config: ChannelConfigAdapter<ResolvedAccount>;
  configSchema?: ChannelConfigSchema;
  setup?: ChannelSetupAdapter;
  pairing?: ChannelPairingAdapter;
  security?: ChannelSecurityAdapter;
  groups?: ChannelGroupAdapter;
  mentions?: ChannelMentionAdapter;
  outbound?: ChannelOutboundAdapter;
  status?: ChannelStatusAdapter;
  gateway?: ChannelGatewayAdapter;
  auth?: ChannelAuthAdapter;
  approvalCapability?: ChannelApprovalCapability;
  elevated?: ChannelElevatedAdapter;
  commands?: ChannelCommandAdapter;
  lifecycle?: ChannelLifecycleAdapter;
  secrets?: ChannelSecretsAdapter;
  allowlist?: ChannelAllowlistAdapter;
  doctor?: ChannelDoctorAdapter;
  bindings?: ChannelConfiguredBindingProvider;
  conversationBindings?: ChannelConversationBindingSupport;
  streaming?: ChannelStreamingAdapter;
  threading?: ChannelThreadingAdapter;
  messaging?: ChannelMessagingAdapter;
  agentPrompt?: ChannelAgentPromptAdapter;
  directory?: ChannelDirectoryAdapter;
  resolver?: ChannelResolverAdapter;
  actions?: ChannelMessageActionAdapter;
  heartbeat?: ChannelHeartbeatAdapter;
  agentTools?: ChannelAgentToolFactory | ChannelAgentTool[];
};
```

### Config Adapter 的精细化能力

`ChannelConfigAdapter` 不是简单的配置读取，而是包含 15+ 个方法的完整接口：

- `listAccountIds()` — 列出所有已配置的 account
- `resolveAccount()` — 解析 account 配置对象
- `inspectAccount()` — 检查 account 状态
- `defaultAccountId()` — 获取默认 account
- `setAccountEnabled()` — 启用/禁用 account
- `deleteAccount()` — 删除 account
- `isEnabled()` — 判断 account 是否启用
- `disabledReason()` — 获取禁用原因
- `isConfigured()` — 判断 account 是否已配置
- `unconfiguredReason()` — 获取未配置原因
- `describeAccount()` — 生成 account 快照
- `resolveAllowFrom()` — 解析允许列表
- `formatAllowFrom()` — 格式化允许列表
- `hasConfiguredState()` — 检查是否有持久化配置
- `hasPersistedAuthState()` — 检查是否有持久化认证
- `resolveDefaultTo()` — 解析默认目标

### Setup Adapter 的向导式配置

`ChannelSetupAdapter` 支持向导式的 channel 配置：

- `resolveAccountId()` — 从输入解析 account ID
- `resolveBindingAccountId()` — 解析绑定 account
- `applyAccountName()` — 应用 account 名称
- `applyAccountConfig()` — 应用 account 配置
- `afterAccountConfigWritten()` — 配置写入后的回调
- `validateInput()` — 输入验证
- `singleAccountKeysToMove()` — 单 account 迁移 key
- `namedAccountPromotionKeys()` — 命名 account 提升 key

---

## 2. Lifecycle 管理 — 全生命周期状态机

### Typing Lifecycle

`src/channels/typing-lifecycle.ts` 实现了输入指示器的生命周期管理：

```typescript
type TypingKeepaliveLoop = {
  tick: () => Promise<void>;
  start: () => void;
  stop: () => void;
  isRunning: () => boolean;
};
```

- 创建定时循环保持 typing 状态
- 防止并发 tick（`tickInFlight` 标志）
- 支持 start/stop 控制

### Status Reactions Lifecycle

`status-reactions.slack-lifecycle.ts` 处理 Slack 的状态反应（emoji reaction）生命周期。

### Transport Stall Watchdog

`src/channels/transport/stall-watchdog.ts` 监控传输层的卡死状态：
- 检测长时间无响应的传输连接
- 触发重连或告警

---

## 3. Policy 系统 — 多层策略控制

### Mention Gating（提及门控）

`src/channels/mention-gating.ts` 实现了精细的提及检测策略：

```typescript
type InboundImplicitMentionKind =
  | "reply_to_bot"      // 回复 bot 消息
  | "quoted_bot"        // 引用 bot 消息
  | "bot_thread_participant" // bot 是 thread 参与者
  | "native";           // 原生提及

type InboundMentionFacts = {
  canDetectMention: boolean;
  wasMentioned: boolean;
  hasAnyMention?: boolean;
  implicitMentionKinds?: readonly InboundImplicitMentionKind[];
};

type InboundMentionPolicy = {
  isGroup: boolean;
  requireMention: boolean;
  allowedImplicitMentionKinds?: readonly InboundImplicitMentionKind[];
  allowTextCommands: boolean;
  hasControlCommand: boolean;
  commandAuthorized: boolean;
};

type InboundMentionDecision = {
  effectiveWasMentioned: boolean;
  shouldSkip: boolean;
  implicitMention: boolean;
  matchedImplicitMentionKinds: InboundImplicitMentionKind[];
  shouldBypassMention: boolean;
};
```

**决策逻辑**：
- 在群组中是否需要显式提及才能响应
- 支持隐式提及检测（回复、引用、thread 参与）
- 控制命令可以绕过提及要求
- 区分"跳过"和"绕过提及"两种行为

### Command Gating（命令门控）

`src/channels/command-gating.ts` 实现了基于访问组的命令权限控制：

```typescript
type CommandAuthorizer = {
  configured: boolean;  // 是否已配置
  allowed: boolean;     // 是否允许
};

type CommandGatingModeWhenAccessGroupsOff = "allow" | "deny" | "configured";
```

**决策逻辑**：
- 当访问组关闭时，支持三种模式：全部允许、全部拒绝、按配置决定
- 多 authorizer 的 OR 逻辑
- `resolveControlCommandGate()` — 控制命令是否被阻塞
- `resolveDualTextControlCommandGate()` — 支持主次 authorizer

### Inbound Debounce Policy（入站防抖策略）

`src/channels/inbound-debounce-policy.ts` 实现了消息防抖机制：

```typescript
function shouldDebounceTextInbound(params: {
  text: string | null | undefined;
  cfg: OpenClawConfig;
  hasMedia?: boolean;
  commandOptions?: CommandNormalizeOptions;
  allowDebounce?: boolean;
}): boolean;
```

- 含有控制命令的消息不防抖
- 含有媒体的消息不防抖
- 可配置 debounce 时间窗口
- 按 channel 独立配置

### Thread Binding Policy（线程绑定策略）

`src/channels/thread-bindings-policy.ts` 实现了会话绑定的生命周期策略：

```typescript
type ThreadBindingSpawnKind = "subagent" | "acp";

type ThreadBindingSpawnPolicy = {
  channel: string;
  accountId: string;
  enabled: boolean;
  spawnEnabled: boolean;
};
```

- **Idle Timeout**：默认 24 小时无活动后解除绑定
- **Max Age**：最大绑定时长（默认 0 = 无限制）
- **Placement**：`current`（当前 thread）或 `child`（子 thread）
- **Spawn**：支持自动 spawn subagent 或 ACP session
- 支持 channel 级别和 account 级别的独立配置

---

## 4. Conversation Resolution — 会话解析系统

### 多来源会话解析

`src/channels/conversation-resolution.ts` 实现了复杂的会话目标解析：

```typescript
type ConversationResolutionSource =
  | "command-provider"           // 命令提供者解析
  | "focused-binding"            // 聚焦绑定
  | "command-fallback"           // 命令回退
  | "inbound-provider"           // 入站提供者解析
  | "inbound-bundled-artifact"   // 入站内置 artifact
  | "inbound-bundled-plugin"     // 入站内置插件
  | "inbound-fallback";          // 入站回退

type ConversationResolution = {
  canonical: {
    channel: string;
    accountId: string;
    conversationId: string;
    parentConversationId?: string;
  };
  threadId?: string;
  placementHint?: "current" | "child";
  source: ConversationResolutionSource;
};
```

**解析流程**：
1. 首先尝试从 channel plugin 的 adapter 解析
2. 回退到内置 bundled channel 的 artifact 解析
3. 回退到通用 fallback 解析
4. 支持多种输入参数：`commandTo`、`fallbackTo`、`from`、`nativeChannelId`

### Channel Config 匹配

`src/channels/channel-config.ts` 实现了多层级配置匹配：

```typescript
type ChannelEntryMatch<T> = {
  entry?: T;
  key?: string;
  wildcardEntry?: T;
  wildcardKey?: string;
  parentEntry?: T;
  parentKey?: string;
  matchKey?: string;
  matchSource?: "direct" | "parent" | "wildcard";
};
```

**匹配优先级**：
1. Direct match — 精确匹配
2. Normalized match — 规范化后匹配
3. Parent match — 父级 fallback
4. Wildcard match — 通配符 fallback

支持 `buildChannelKeyCandidates()` 构建多种候选 key，以及 `normalizeChannelSlug()` 规范化 channel slug。

---

## 5. Sender Identity — 发送者身份验证

`src/channels/sender-identity.ts` 实现了严格的发送者身份验证：

```typescript
function validateSenderIdentity(ctx: MsgContext): string[] {
  const chatType = normalizeChatType(ctx.ChatType);
  const isDirect = chatType === "direct";

  const senderId = normalizeOptionalString(ctx.SenderId) || "";
  const senderName = normalizeOptionalString(ctx.SenderName) || "";
  const senderUsername = normalizeOptionalString(ctx.SenderUsername) || "";
  const senderE164 = normalizeOptionalString(ctx.SenderE164) || "";

  if (!isDirect) {
    if (!senderId && !senderName && !senderUsername && !senderE164) {
      issues.push("missing sender identity");
    }
  }

  if (senderE164) {
    if (!/^\+\d{3,}$/.test(senderE164)) {
      issues.push(`invalid SenderE164: ${senderE164}`);
    }
  }

  if (senderUsername) {
    if (senderUsername.includes("@")) {
      issues.push(`SenderUsername should not include "@"`);
    }
    if (/\s/.test(senderUsername)) {
      issues.push(`SenderUsername should not include whitespace`);
    }
  }
}
```

**验证规则**：
- 非私聊必须至少有一个发送者标识
- E164 号码格式验证（`+` 开头，至少3位数字）
- Username 不能包含 `@` 和空白字符
- SenderId 设置但不能为空

---

## 6. Session Envelope — 会话信封上下文

`src/channels/session-envelope.ts` 实现了入站消息的会话信封解析：

```typescript
function resolveInboundSessionEnvelopeContext(params: {
  cfg: OpenClawConfig;
  agentId: string;
  sessionKey: string;
}) {
  const storePath = resolveStorePath(params.cfg.session?.store, {
    agentId: params.agentId,
  });
  return {
    storePath,
    envelopeOptions: resolveEnvelopeFormatOptions(params.cfg),
    previousTimestamp: readSessionUpdatedAt({
      storePath,
      sessionKey: params.sessionKey,
    }),
  };
}
```

- 解析会话存储路径
- 读取信封格式选项
- 获取上次更新时间戳（用于计算会话间隔）

---

## 7. Reply Prefix — 回复前缀模板系统

`src/channels/reply-prefix.ts` 实现了动态回复前缀生成：

```typescript
type ReplyPrefixContextBundle = {
  prefixContext: ResponsePrefixContext;
  responsePrefix?: string;
  responsePrefixContextProvider: () => ResponsePrefixContext;
  onModelSelected: (ctx: ModelSelectionContext) => void;
};
```

- 支持在回复前添加前缀（如模型名称、思考级别）
- 前缀上下文动态更新（provider、model、thinkingLevel）
- 通过 `onModelSelected` 回调在模型选择后更新前缀

---

## 8. Channel Allowlists — 允许列表系统

`src/channels/allowlists/` 实现了多来源的允许列表匹配：

- `allowlist-match.ts` — 允许列表匹配逻辑
- `allow-from.ts` — 允许来源解析
- `resolve-utils.ts` — 解析工具

支持按 account、channel、group 维度配置允许列表。

---

## 9. Account Snapshot — 账户快照系统

`src/channels/account-snapshot-fields.ts` 和 `account-summary.ts` 实现了 channel account 的快照和摘要：

- 生成 channel account 的状态快照
- 支持诊断信息的格式化展示
- 支持 `ChannelCapabilitiesDisplayLine` 的 tone 控制（default/muted/success/warn/error）

---

## 10. ACP Bindings — ACP 绑定集成

`src/channels/plugins/acp-bindings.ts` 实现了 channel 与 ACP 协议的绑定集成：

- `acp-configured-binding-consumer.ts` — 消费配置化的绑定
- `acp-stateful-target-driver.ts` — 有状态的目标驱动
- `acp-stateful-target-reset.runtime.ts` — 有状态目标重置

支持 channel 级别的 ACP 会话自动绑定和重置。

---

## 总结对比

| 机制 | Manta（当前） | OpenClaw |
|------|-------------|----------|
| **架构** | 独立模块，feature-gated | Channel Plugin 架构，20+ adapter |
| **Config** | 简单配置读取 | 15+ 方法的 ConfigAdapter + 向导式 Setup |
| **Lifecycle** | 基本的连接/断开 | Typing、Status、Heartbeat、Transport Watchdog |
| **Policy** | 基础 mention 检测 | Mention Gating + Command Gating + Debounce + Thread Binding |
| **Conversation** | 简单的 channel → session 映射 | 6 种来源的多层解析 + Placement Hint |
| **Identity** | 基础 sender ID | 严格的多字段验证（ID/Name/Username/E164） |
| **Envelope** | 无 | Session Envelope 上下文（storePath + timestamp） |
| **Reply** | 直接发送 | Reply Prefix 模板系统（动态模型信息） |
| **Allowlist** | 基础 allowlist | 多维度允许列表匹配 |
| **Account** | 无 | Account Snapshot + 诊断展示 |
| **ACP** | 无 | Channel-ACP 绑定集成 |

---

## 对 Manta 的借鉴建议

### 短期

1. **Channel Config Adapter**
   - 将 channel 配置抽象为 `ChannelConfig` trait，支持 list/resolve/inspect/enabled 等方法
   - 在 TOML 配置中支持 account 级别的独立配置
   - 实现 account 的启用/禁用/删除操作

2. **Mention Gating**
   - 在 `src/channels/` 中实现 `mention_gate.rs`（已有基础）
   - 支持隐式提及检测（回复、引用、thread 参与）
   - 区分群组/私聊的不同策略

3. **Inbound Debounce**
   - 实现消息防抖机制，避免快速重复触发
   - 按 channel 配置 debounce 时间
   - 命令消息跳过防抖

4. **Sender Identity 验证**
   - 严格验证 sender ID、name、username 的格式
   - 支持 E164 号码验证
   - 非私聊场景要求至少一个标识

### 中期

5. **Conversation Resolution**
   - 实现多来源的会话解析（命令/入站/绑定/fallback）
   - 支持 thread ID 和 parent conversation ID
   - 支持 placement hint（current/child）

6. **Thread Binding Policy**
   - 实现会话绑定的生命周期管理
   - 支持 idle timeout 和 max age
   - 支持绑定到当前 thread 或子 thread

7. **Reply Prefix**
   - 支持动态回复前缀模板
   - 前缀内容随模型选择动态更新
   - 可配置的前缀格式

8. **Account Snapshot**
   - 实现 channel account 的状态快照
   - 支持诊断信息的格式化展示
   - 集成到 `manta status` 命令

### 长期

9. **Channel Plugin 架构**
   - 将 channel 抽象为 WASM 插件
   - 定义 `ChannelPlugin` trait，包含 lifecycle、config、outbound 等 adapter
   - 支持第三方 channel 插件

10. **ACP 绑定集成**
    - 实现 channel 与 ACP 协议的绑定
    - 支持 channel 级别的 ACP 会话自动管理
    - 支持有状态的目标驱动

---

## 参考代码位置（OpenClaw）

| 文件 | 职责 |
|------|------|
| `src/channels/plugins/types.plugin.ts` | ChannelPlugin 完整类型定义 |
| `src/channels/plugins/types.adapters.ts` | 所有 adapter 接口定义 |
| `src/channels/typing-lifecycle.ts` | Typing 生命周期管理 |
| `src/channels/mention-gating.ts` | Mention 门控策略 |
| `src/channels/command-gating.ts` | 命令门控策略 |
| `src/channels/inbound-debounce-policy.ts` | 入站防抖策略 |
| `src/channels/thread-bindings-policy.ts` | Thread 绑定策略 |
| `src/channels/conversation-resolution.ts` | 会话解析系统 |
| `src/channels/channel-config.ts` | 配置匹配系统 |
| `src/channels/sender-identity.ts` | 发送者身份验证 |
| `src/channels/session-envelope.ts` | 会话信封上下文 |
| `src/channels/reply-prefix.ts` | 回复前缀模板 |
| `src/channels/allowlists/` | 允许列表系统 |
| `src/channels/account-snapshot-fields.ts` | 账户快照字段 |
| `src/channels/transport/stall-watchdog.ts` | 传输卡死监控 |
| `src/channels/plugins/acp-bindings.ts` | ACP 绑定集成 |
