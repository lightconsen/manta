# ACP 协议实现状态与缺口

## 背景

本文档追踪 Manta ACP（Agent Control Plane）模块的实现状态，参考 OpenClaw 的 ACP 架构进行对齐。Manta 的 ACP 核心代码位于 `src/acp/`，HTTP API 位于 `src/gateway/mod.rs`（`6214+`），工具接口位于 `src/tools/acp_tool.rs`。

---

## Manta ACP 当前实现状态

### 已实现 ✅

| 能力 | 说明 | 代码位置 |
|------|------|----------|
| **Subagent Spawn** | `AcpControlPlane::spawn_subagent()` 支持 `Run` 和 `Session` 两种模式 | `src/acp/mod.rs:929` |
| **Session 串行执行队列** | `acp_actor_loop` → `session_actor_loop`，每个 session 一个 `mpsc` 队列保证 turn 串行执行 | `src/acp/mod.rs:344` |
| **运行时控制** | `pause` / `resume` / `step` / `cancel` 四个控制命令 | `src/acp/mod.rs:802` |
| **HTTP API** | 12 个 REST 端点已注册到 gateway router，受 `config.acp.enabled` 控制 | `src/gateway/mod.rs:1987` |
| **Session 持久化** | `acp_sessions` SQLite 表，daemon 重启后自动恢复 session 列表 | `src/agent/session_store.rs:412+` |
| **审计日志** | `AcpSpawn` / `AcpTerminate` / `AcpMessage` 事件类型，记录到 `audit_log` | `src/security/runtime_audit.rs:37` |
| **Rate Limiting** | `acp_spawn` 端点限制 10 spawns/分钟/actor | `src/gateway/mod.rs:6288` |
| **max_iter 可配置** | `AcpConfig.max_iterations`（默认 50），贯穿 actor loop 和 subagent loop | `src/gateway/mod.rs:235` |
| **Thread Binding** | `New` / `Parent` / `Thread(id)` / `Auto` 四种绑定模式 | `src/acp/mod.rs:64` |
| **Session 状态机** | `Idle` / `Running` / `Paused` / `Stepping` / `Cancelling` / `Completed` | `src/acp/mod.rs:92` |

### WebSocket 子协议方法（`/ws`）

ACP 操作通过 WebSocket RPC 调用，需 `acp` scope：

```
acp.list                # list_acp_sessions_handler → handle_acp_list
acp.spawn               # acp_spawn_handler → handle_acp_spawn
acp.terminate           # terminate_acp_session_handler → handle_acp_terminate
acp.message             # acp_session_message_handler → handle_acp_message
acp.status              # acp_session_status_handler → handle_acp_status
acp.pause               # acp_session_pause_handler → handle_acp_pause
acp.resume              # acp_session_resume_handler → handle_acp_resume
acp.step                # acp_session_step_handler → handle_acp_step
acp.cancel              # acp_session_cancel_handler → handle_acp_cancel
acp.tree                # acp_session_tree_handler → handle_acp_tree
acp.execute.session     # acp_execute_session_handler → handle_acp_execute_session
acp.execute.run         # acp_execute_run_handler → handle_acp_execute_run
```

> 注：所有 `/api/v1/acp/*` REST 端点及 `/chat`、`/api/v1/*` 等 deprecated 路由已从 `build_router()` 中移除，仅保留 WebSocket 协议入口。

### 仍缺失 ❌

| 能力 | 缺口说明 | 优先级 |
|------|----------|--------|
| **ACP 事件流协议** | 没有 `text_delta` / `tool_call` / `usage_update` 等 SSE/WebSocket 事件流输出 | 高 |
| **Spawn 深度限制** | `delegate_tool` 有 depth limiting（`MAX_DEPTH=2`），但 ACP spawn 没有深度限制 | 中 |
| **父子流 Relay** | 子 agent 的输出不会 relay 到父 session 的流中 | 中 |
| **运行时配置热更新** | 没有 `session/set_mode`、`session/set_config_option` 等动态配置接口 | 低 |
| **细粒度 ACP 策略** | 当前只有全局 `acp.enabled` + rate limit，没有按 agent/channel/user 的权限控制 | 低 |
| **OpenClaw 协议兼容** | 事件类型和命名规范未对齐 | 长期 |
| **分布式 ACP** | 多节点 session 一致性未考虑 | 长期 |

---

## OpenClaw ACP 架构（参考）

### 协议分层

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

### Session 生命周期状态机

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

### 事件流协议

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

### 运行时控制命令

ACP 支持双向控制：

```typescript
type AcpRuntimeControl =
  | "session/set_mode"      // 切换 prompt/steer 模式
  | "session/set_config_option"  // 动态调整运行时配置
  | "session/status";       // 查询当前状态
```

### 持久化绑定

`src/acp/persistent-bindings.lifecycle.ts` 实现跨会话的 agent 绑定：

- 通过 `persistent-bindings.resolve.ts` 解析绑定目标
- 支持 thread 级别的 agent 绑定（`thread-bindings-policy.ts`）
- 绑定信息持久化到 session store

### 策略控制

`src/acp/policy.ts` 定义了 ACP 的权限策略：

- `isAcpEnabledByPolicy()`：检查当前配置是否允许 ACP
- `resolveAcpAgentPolicyError()`：当 agent 不符合 ACP 策略时返回错误
- 支持按 agent、channel、用户维度控制 ACP 权限

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
