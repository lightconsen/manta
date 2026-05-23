# Issue: ACP 协议的完整实现（Agent 间协作）

## 背景

Manta 当前在 `src/acp/` 模块中有基础结构，但缺乏完整的 Agent 间协作协议。OpenClaw 的 ACP（Agent Client Protocol）实现了一套成熟的运行时协议，支持 agent 的 spawn、生命周期管理、流式通信和持久化绑定。这套机制是 OpenClaw 实现多 agent 协作和 subagent 调度的核心。

---

## OpenClaw 的 ACP 架构

### 1. 协议分层

OpenClaw 的 ACP 实现分为三层：

#### 控制层（Control Plane）
`src/acp/control-plane/` 负责 session 的全生命周期管理：

- **Manager Core** (`manager.core.ts`)：核心状态机，管理 session 的初始化、turn 执行、关闭
- **身份协调** (`manager.identity-reconcile.ts`)：处理 session identity 的持久化和恢复
- **运行时控制** (`manager.runtime-controls.ts`)：处理 `session/set_mode`、`session/set_config_option`、`session/status` 等控制命令
- **Turn 流处理** (`manager.turn-stream.ts`)：消费 ACP runtime 的流式输出

#### 运行时层（Runtime）
`src/acp/runtime/` 定义了 ACP 协议的消息类型和交互契约：

```typescript
type AcpRuntimeSessionMode = "persistent" | "oneshot";
type AcpRuntimePromptMode = "prompt" | "steer";

type AcpRuntimeHandle = {
  sessionKey: string;
  backend: string;
  runtimeSessionName: string;
  cwd?: string;
  acpxRecordId?: string;
  backendSessionId?: string;
  agentSessionId?: string;
};
```

- **Session Identity** (`runtime/session-identity.ts`)：管理 session 的唯一标识和恢复逻辑
- **错误体系** (`runtime/errors.ts`)：定义 `AcpRuntimeError` 及错误边界处理
- **运行时选项** (`runtime-options.ts`)：模型覆盖、思考模式等运行时配置验证

#### 适配层（Spawn / Binding）
`src/agents/acp-spawn.ts` 实现 subagent 的 spawn 逻辑：

- 创建子 agent 的 ACP session
- 继承父 agent 的 workspace 和上下文
- 设置 spawn 深度限制（`DEFAULT_SUBAGENT_MAX_SPAWN_DEPTH`）
- 处理子 agent 的 stream relay（`acp-spawn-parent-stream.ts`）

### 2. Session 生命周期状态机

```
InitializeSession
  → Ensure Runtime Handle
    → Run Turn (prompt/steer)
      → Stream Events (text_delta, status, tool_call, ...)
        → Turn Complete
          → [Idle Timeout] → Close Session
          → [New Turn] → Run Turn
```

Manager 使用 `RuntimeCache` 缓存活跃 runtime，并通过 `SessionActorQueue` 保证同一 session 的 turn 串行执行。

### 3. 事件流协议

ACP 定义了丰富的运行时事件类型：

```typescript
type AcpSessionUpdateTag =
  | "agent_message_chunk"
  | "agent_thought_chunk"
  | "tool_call"
  | "tool_call_update"
  | "usage_update"
  | "available_commands_update"
  | "current_mode_update"
  | "config_option_update"
  | "session_info_update"
  | "plan"
  | (string & {});  // 扩展点
```

事件通过流式接口推送，支持：
- **文本增量** (`text_delta`)：标准输出和思考流
- **工具调用** (`tool_call`)：工具调用开始/更新/完成
- **状态更新** (`status`)：运行时状态摘要
- **使用统计** (`usage_update`)：token 消耗

### 4. 运行时控制命令

ACP 支持双向控制：

```typescript
type AcpRuntimeControl =
  | "session/set_mode"      // 切换 prompt/steer 模式
  | "session/set_config_option"  // 动态调整运行时配置
  | "session/status";       // 查询当前状态
```

### 5. 持久化绑定

`src/acp/persistent-bindings.lifecycle.ts` 实现跨会话的 agent 绑定：

- 通过 `persistent-bindings.resolve.ts` 解析绑定目标
- 支持 thread 级别的 agent 绑定（`thread-bindings-policy.ts`）
- 绑定信息持久化到 session store

### 6. 与 Agent Spawn 的集成

```typescript
// src/agents/acp-spawn.ts 的核心流程
async function spawnSubagent(params: {
  agent: string;
  sessionKey: string;
  mode: AcpRuntimeSessionMode;
  requesterOrigin: string;
  workspaceInheritance: WorkspaceInheritance;
}): Promise<AcpSpawnResult> {
  // 1. 检查 ACP 策略是否允许 spawn
  // 2. 解析会话工作目录
  // 3. 创建父流 relay（将子 agent 输出 relay 到父会话）
  // 4. 调用 Gateway 创建 ACP session
  // 5. 注册到 ACP Session Manager
  // 6. 启动后台任务追踪
}
```

### 7. 策略控制

`src/acp/policy.ts` 定义了 ACP 的权限策略：

- `isAcpEnabledByPolicy()`：检查当前配置是否允许 ACP
- `resolveAcpAgentPolicyError()`：当 agent 不符合 ACP 策略时返回错误
- 支持按 agent、channel、用户维度控制 ACP 权限

---

## 对 Manta 的借鉴建议

### 短期（协议设计）

1. **定义 ACP 核心消息类型**
   - 在 `src/acp/` 中定义 Rust 版的 `AcpRuntimeHandle`、`AcpRuntimeTurnInput`、`AcpRuntimeEvent`
   - 使用 `serde` 序列化，确保跨语言兼容性
   - 设计事件流接口（类似 SSE 或 WebSocket）

2. **Session Manager 状态机**
   - 实现 `AcpSessionManager` struct，管理 session 生命周期
   - 使用 `tokio::sync::RwLock` 或 `dashmap` 管理活跃 session 缓存
   - 实现 turn 队列保证串行执行

3. **Agent Spawn 基础**
   - 在 `src/agent/` 中实现 `spawn_subagent()` 函数
   - 限制 spawn 深度和子 agent 数量（防止资源耗尽）
   - 实现父子 agent 的 stream relay

### 中期（运行时能力）

4. **运行时控制协议**
   - 实现 `session/set_mode`、`session/set_config_option`、`session/status`
   - 支持 prompt/steer 模式切换（steer 模式用于中途干预 agent）
   - 运行时配置热更新（模型覆盖、思考模式等）

5. **持久化绑定**
   - 实现 thread/channel 级别的 agent 持久化绑定
   - 绑定信息存入 SQLite（`src/memory/sqlite.rs`）
   - 支持绑定过期策略（idle timeout、max age）

6. **策略与权限**
   - 在 `src/security/` 中增加 ACP 策略模块
   - 支持按 agent ID、channel、用户控制 spawn 权限
   - 与现有的 `mention_gate.rs` 和 `pairing.rs` 集成

### 长期（生态兼容）

7. **与 OpenClaw ACP 协议兼容**
   - 对齐事件类型和命名规范，实现互操作
   - 考虑支持 ACPX（OpenClaw 的 ACP 扩展协议）
   - 为跨平台 agent 协作奠定基础

8. **分布式 ACP**
   - 当 Manta 的 Gateway 支持多节点时，实现分布式 session 管理
   - 使用 WebSocket 或 gRPC 进行跨节点 ACP 通信
   - session identity 的分布式一致性

---

## 参考代码位置（OpenClaw）

| 文件 | 职责 |
|------|------|
| `src/acp/control-plane/manager.core.ts` | Session 生命周期状态机核心 |
| `src/acp/control-plane/manager.identity-reconcile.ts` | Session identity 持久化与恢复 |
| `src/acp/control-plane/manager.runtime-controls.ts` | 运行时控制命令处理 |
| `src/acp/control-plane/manager.turn-stream.ts` | Turn 流消费与事件分发 |
| `src/acp/runtime/types.ts` | ACP 协议类型定义 |
| `src/acp/runtime/session-identity.ts` | Session identity 管理 |
| `src/acp/runtime/errors.ts` | ACP 运行时错误体系 |
| `src/agents/acp-spawn.ts` | Subagent spawn 实现 |
| `src/agents/acp-spawn-parent-stream.ts` | 父-子 agent 流 relay |
| `src/acp/persistent-bindings.lifecycle.ts` | 持久化绑定生命周期 |
| `src/acp/policy.ts` | ACP 权限策略 |
