# Goal-Based Execution (`/goal`)

`/goal` runs an agent autonomously until structured stop conditions are met.
Unlike turn-based chat where the user drives each round, the goal runner
iterates — agent acts, checks all conditions, feeds back failures, retries —
until all pass or a guardrail trips.

---

## Quick Start

```
/goal 给 src/module.rs 写测试，覆盖所有公有函数 --max-rounds 3
```

response: `🎯 Goal started: ... ID: goal_<uuid>`

Events stream back to the session WebSocket as the goal progresses.

---

## Subcommands

| Command | Description |
|---|---|
| `/goal <description> [--max-rounds N]` | Start a new goal |
| `/goal cancel <goal_id>` | Cancel a running goal |
| `/goal list` | List active goals |

---

## How It Works

```
User: /goal 给 module.rs 写测试
  ↓
LLM 解析目标 → GoalPlan {
    description: "给 module.rs 写测试",
    conditions: [
      { type: "file_exists", path: "tests/module.rs" },
      { type: "exit_code", command: "cargo test" },
      { type: "numeric", command: "grep -c 'fn test_' tests/module.rs", operator: ">=", threshold: 5 }
    ],
    max_rounds: 3
  }
  ↓
GoalRunner 循环:
  round 1: agent 行动 → 检查条件 → 2/3 通过 → 反馈失败项
  round 2: agent 修复 → 检查条件 → 3/3 通过 → ✅ Done
  (或达到 max_rounds / 检测到死循环 → ❌ Aborted)
```

### Event Sequence

Each goal emits `goal.progress` gateway events with a nested `event` field:

```
goal.started → goal.retry → goal.check → (goal.done | goal.aborted)
```

- **goal.started**: goal 已创建，包含描述和条件列表
- **goal.retry**: 新一轮开始，包含上一轮反馈
- **goal.check**: 条件检查结果，含通过数/总数
- **goal.done**: 所有条件通过
- **goal.aborted**: 达到 max_rounds / 死循环 / 手动取消 / 错误

---

## Condition Types

LLM 根据目标自动生成条件。支持的类型：

| Type | JSON | 检查方式 |
|---|---|---|
| `exit_code` | `{"type":"exit_code","command":"cargo test","expected":0}` | 命令退出码 |
| `file_exists` | `{"type":"file_exists","path":"tests/module.rs"}` | 文件存在性 |
| `numeric` | `{"type":"numeric","command":"wc -l","operator":">=","threshold":5}` | 命令输出数值比较 |
| `pattern` | `{"type":"pattern","command":"cat src/lib.rs","must_contain":"fn test_"}` | 输出包含字符串 |
| `static_analysis` | `{"type":"static_analysis","command":"cargo clippy -- -D warnings"}` | 退出码 0 |

### 条件评估

所有条件是 AND 关系——全部通过才算达标。检查过程是确定性的（无 LLM 调用），直接在本地执行命令。

---

## Safety Guardrails

| Guardrail | Default | 说明 |
|---|---|---|
| `max_rounds` | 5 | 最大重试轮数，`--max-rounds` 可覆盖 |
| Loop detection | 3 rounds | 同一条件连续 3 轮相同失败原因 → abort |
| Cancellation | — | `/goal cancel <id>` 手动中止 |
| Tool iterations | 25 | 单轮内最大 LLM→tool 调用次数 |

---

## Model Override

GoalPlan 支持 `model_override` 字段，LLM 可以在返回的计划中指定子 agent 使用低成本模型：

```json
{
  "description": "code review",
  "conditions": [...],
  "max_rounds": 3,
  "model_override": "gpt-4o-mini"
}
```

未指定时使用 session 默认模型。

---

## Persistence

Goal 状态在每个 round 结束后 checkpoint 到 `~/.syscity/goals/<goal_id>.json`。
Gateway 启动时自动恢复所有 persisted goal，从上一次 round 继续执行。

Goal 完成后或取消时状态文件自动删除。

---

## Architecture

```
src/goal/
├── mod.rs          # 模块入口，重导出
├── condition.rs    # GoalCondition enum + check() 执行
├── plan.rs         # GoalPlan + LLM 解析
├── runner.rs       # GoalRunner 子 agent 执行循环
├── event.rs        # GoalEvent 事件类型
└── persist.rs      # GoalStore 持久化/恢复
```

Gateway 集成：

- `src/gateway/commands.rs` — `/goal` 命令处理
- `src/gateway/lifecycle.rs` — 启动时恢复 persisted goals
- `src/gateway/protocol.rs` — `gateway.progress` 事件序列化
- `src/gateway/ws.rs` — WebSocket 路由到订阅 session

---

## 与服务模式的对应关系

| 模式 | 对应关系 |
|---|---|
| Turn-based | 默认聊天，用户驱动每轮 |
| Goal-based | `/goal`，条件驱动终止 |
| Time-based | `/schedule` / cron / heartbeat |
| Proactive | 事件驱动编排（未实现） |

Goal-based 填补了 turn-based 和 time-based 之间的空白：不需要用户每轮点击发送，也不需要定时器，而是"做完就算完"。
