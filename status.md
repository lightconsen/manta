# Syscity 模块代码审查报告

审查范围：`docs/modules/` 下 36 个模块文档及对应 `src/` 实现。
评估维度：功能完整度、系统集成、代码优化、prompt 优化、架构、冗余/死代码。
日期：2026-06-18。

---

## 一、按集成状态分级

### 🟢 已完整集成且活跃使用（核心路径）

| 模块 | 完成度 | 集成点 | 备注 |
|------|--------|--------|------|
| `gateway` | ✓ | 控制平面入口 | 已于 2026-06-20 拆分为 lifecycle/dispatch/hot_reload/init·channels/runtime 子模块，mod.rs 1077 行 |
| `agent` | ✓ | gateway 创建 | 模块面广但分文件清晰 |
| `tools` | ✓ | 全系统使用 | RBAC/审批/熔断已就绪 |
| `providers` | ✓ | 协议三合一架构整洁 | 已成功淘汰 ollama/moonshot/minimax 单文件 |
| `model_router` | ✓ | gateway/agent | 与 providers 边界清晰 |
| `memory` | ✓ | agent + gateway | 支持多后端，dreaming 已运行 |
| `perception` | ✓ | gateway 启动 + 每 agent | 流水线 + 三种 summarizer 完成 |
| `device` | ✓ | gateway 启动 | 真实驱动 + 原生插件 + OS bridge 全可用 |
| `computer` | ✓ | agent 桌面任务 | 跨平台 adapter 齐全 |
| `channels` | ✓ | gateway 多通道 | feature gate 严格 |
| `inbound` | ✓ | gateway 消息入口 | 7 阶段流水线均落地 |
| `outbound` | ✓ | gateway 响应出口 | 已注入 trajectory/SSE/canvas/dispatcher |
| `acp` | ✓ | gateway + tools | 子代理控制平面就绪 |
| `mcp` | ✓ | tools 动态注册 | stdio/sse/http 三个传输齐全 |
| `plugins` | ✓ | gateway 启动 | WASM 沙箱 + 热重载 + provider/channel 扩展 |
| `skills` | ✓ | agent prompt | 多级存储 + 热加载 |
| `cli` | ✓ | binary | 20+ 子命令，覆盖完整 |
| `daemon` | ✓ | binary | start/stop/reload/status 完备 |
| `config` | ✓ | 全局 | hot-reload + secret 解析 |
| `security` | ✓ | gateway 中间件 | auth/allowlist/rate-limit 多层 |
| `capabilities` | ✓ | gateway 启动 | 5 种 profile + 平台约束 |
| `tui` | ✓ | binary 子命令 | ratatui/crossterm 完整实现 |
| `cron` | ✓ | gateway 启动 + tool | 定时任务核心 |
| `heartbeat` | ✓ | gateway 启动 | 周期 wake + 活跃时段 |
| `standing_orders` | ✓ | gateway 启动 | cron-like 后台 agent |
| `tailscale` | ✓ | gateway 启动 | serve / funnel 模式 |
| `export` | ✓ | CLI export | md/json/jsonl |
| `core` | ✓ | 全局 | Id / Engine / EventBus 基础 |
| `utils` | ✓ | 全局 | batch/pool/profiling/logging |
| `adapters` | ✓ | gateway/init/storage | InMem/File/Sqlite 三后端 |
| `canvas` | ✓ | outbound 注入 | A2UI 16 种组件 |

### 🟡 已实现但仅边缘集成

> ~~`team`~~ 已于 2026-06-18 删除（1647 行 + tool 397 行,功能与 ACP/delegate 重叠且未注册到 ToolRegistry,实质死代码）。
> ~~`eval`~~ 已于 2026-06-18 删除（536 行,零外部使用,无 CLI/CI 接线;需要时再重写）。

### 🔴 实现完整但完全未接线（死代码风险）

> ~~`flow` / `taskflow`~~ 已于 2026-06-18 删除（功能被 `src/planner/` 吸收）。
> ~~`server`~~ 已于 2026-06-18 删除（598 行,零外部使用,功能完全被 `gateway` 覆盖）。

---

## 二、架构层面观察

### 1. ~~`gateway/mod.rs` 过厚~~（已解决，2026-06-20）

* ~~单文件 3000+ 行，承载启动序列、热重载、agent 池、SSE、handlers wiring、多个 free fn 助手。~~
* 已于 2026-06-20 完成拆分，`mod.rs` 从 3642 行降至 1077 行（减少 71%）。抽出的子模块：
  * `gateway/lifecycle.rs`（`start_gateway` / `stop_gateway` / `build_router` 自由函数）
  * `gateway/dispatch.rs`（inbound 消息入口 worker + routed 消息分发）
  * `gateway/hot_reload.rs`（Main/Agent/Channel/Plugin/Gateway 五类 config 变更处理器）
  * `gateway/init/channels.rs`（8 种 channel-type 的 init 函数）
  * `gateway/runtime.rs`（`BufferedMessage` / `AgentHandle` / `AgentCommand` / `AgentQuery` / `GatewayEvent` / `AgentStatus` 运行时类型）
  * 此前已有 `gateway/init/{devices,storage}.rs`、`gateway/{config,state,types,watchdog,agent_spawn}.rs`。

### 2. ~~双服务器入口~~（已解决）

* ~~`src/server/mod.rs` 与 `src/gateway/` 提供两套 HTTP API~~。`src/server/` 已于 2026-06-18 删除；生产入口仅为 `gateway`。

### 3. `flow` vs `taskflow` 重复

* 两套都是 DAG / 检查点执行框架。`flow` 偏审批门 + 失败策略；`taskflow` 偏 SQLite checkpoint。
* `planner` 自带一套 `DagScheduler + RollbackManager`，并实际接入了 `Agent::spawn_with_computer_adapter`。
* 三者并存导致认知负担。**建议**先把 `flow` / `taskflow` 标记 `#[deprecated]` 或迁出 `experimental/`，等真有 RFC 再回归主线。

### 4. ~~`eval` 无 CI 入口~~（已删除）

模块已于 2026-06-18 移除（536 行,零外部使用,无 CLI/CI 接线）。如未来重启回归测试,直接重写。

### 5. ~~`team` 仅供 tool 使用~~（已解决）

* `team` 模块已于 2026-06-18 删除。多 agent 协作走 ACP (`acp_spawn` / `delegate` 工具) 这条主线;`team` 的 mailbox 模型与已存在的 ACP subagent 体系重叠且未真正接线。

### 6. 文档与代码漂移

| 漂移点 | 现状 |
|--------|------|
| `outbound.md` 列 `canvas.rs` 在 outbound 内部 | 实际是 `crate::canvas` 顶层模块，outbound 只 import |
| `outbound.md` `SseEvent { ToolStart, ToolComplete, ContentDelta, Done, Error }` | 实际为 `{ Token, ToolStart, ToolEnd, Done, Error, Heartbeat }` |
| `outbound.md` `TrajectoryEntry { timestamp, action, input, output, duration_ms }` | 实际为 typed enum `Start/ToolCall/ToolResult/LlmCall/Reasoning/Finish/Error` |
| `adapters.md` 暗示 `src/storage/` | 实际全在 `src/adapters/storage.rs`，`src/storage/` 不存在 |
| `docs/os.md` 多处路径 | ~~引用已删除目录~~ 已于 2026-06-20 修正 |
| `cli.md` 列出的 `Commands` 枚举 | 实测有差异；`syscity tui` 等子命令未列出 |
| `gateway.md` 的 `GatewayConfig` 字段表 | 缺 `perception`、`device`、`os_bridge` 等新增字段 |

---

## 三、按维度的问题清单

### 功能完整度

* `flow` / `taskflow`：**功能齐**但**未接线**，等同于 PoC。
* `heartbeat`：doc 提到「cron-like 表达式」，实际仅有活跃时段 + 固定 interval；未发现 cron 解析。
* `eval`：~~缺 LLM-as-a-Judge 实际打分调用器~~（模块已删除,2026-06-18）。
* `tailscale`：`status()` 只返回字符串，无结构化字段；运维侧不便。
* ~~`team`：缺 broadcast 持久化，重启即丢消息~~（已删除）。

### 系统集成

* ~~三大孤岛：`flow`、`taskflow`、`server`~~（均已删除）。
* ~~`team`：处于「集成弱」状态~~（已删除）。

### 代码优化

* ~~`gateway/mod.rs` 拆分（见上文 II.1）~~ 已于 2026-06-20 完成。
* `agent`：`is_desktop_task` / `is_complex_task` 关键词启发式应抽到 `agent/heuristics.rs`，便于针对 i18n 场景维护（已混入中文关键词）。
* `perception`：`PerceptionRegistry` 内部多个 `RwLock<HashMap<...>>`，建议加 `parking_lot` 或者评估 `dashmap`，poll 路径上锁次数较多。
* `providers/preset.rs`：每加一个新 vendor 仍是 hand-coded match 分支，可考虑数据驱动 + `serde` 反序列化保留 hand-rolled 兼容。
* `tools`：`ToolContext` 字段已 20+，可拆分子结构（identity / sandbox / model）；调用者构造越来越冗长。
* `inbound` / `outbound`：两边都各自维护 `*_pipeline.rs` + 多个 stage 文件，但 `Default*Pipeline` 都是 hard-coded 顺序；可以引入小型 stage trait 列表以便插件扩展。

### Prompt 优化

* `agent/prompt_builder`：当前 `## Perception` block 与 `### Recent events / Sensors / Entities` 在小型 LLM 下偏冗长。`enable_summary=false` 是新默认，但 prompt 里仍可以裁剪：
  * 当 `Snapshot.entities` 与 `aggregates` 都为空时，应连标题都不输出（已实现 `format_for_prompt` 返回 None）。✓
  * `### Sensors` 输出是完整 JSON dump，建议截断到 top-N（per modality）以防爆 token。
* `planner/DECOMPOSITION_SYSTEM_PROMPT`：已包含 `tool_call`，但样例少。建议加 1-2 条 device-tool 示例，指明何时优先用 `device_*` vs 通用 `shell`。
* `skills` 触发词：当前 priority 只参与排序，未参与冲突解释；可在 prompt 中加 `Skill triggered:` 注释，方便用户调试。
* `eval`：~~尚无 prompt~~（模块已删除）。

### 架构

* 见 II 节。
* 一个潜在抽象问题：`channels` 的 `MessageFormatter` / `ReplyPrefixEngine` 与 `outbound/reply_dispatcher` 职责模糊；reply prefix 既出现在 `channels` 也出现在 `outbound`，需在某处文档明确「谁负责加 prefix」。

### 冗余/死代码

| 位置 | 性质 |
|------|------|
| ~~`src/flow/`、`src/taskflow/`、`src/server/`、`src/eval/`~~ | 已于 2026-06-18 全部删除 |
| ~~`docs/os.md` 多处路径~~ | 已于 2026-06-20 修正：`src/computer/capabilities/` → `src/computer/platform/`，`CapabilitySet`→`PlatformToolSet`，`CapabilityRegistry`→`PlatformCapabilityRegistry` |
| ~~`Cargo.toml` 中 `tui` feature~~ | 已于 2026-06-20 删除（空 no-op feature，代码无 `#[cfg(feature = "tui")]`，CI/脚本均无引用） |
| `outbound.md` 多处 enum 描述 | 与代码漂移 |

---

## 四、改进建议（按优先级）

### P0（建议本季度内处理）

1. ~~**裁定 flow / taskflow / server 的去留**~~：三者均已删除（2026-06-18）。
2. ~~**拆分 `gateway/mod.rs`**~~：已于 2026-06-20 完成。按 lifecycle / dispatch / hot_reload / init·channels / runtime 五个子模块抽出，`mod.rs` 3642 → 1077 行。
3. **同步文档与代码**：列出的 6 处漂移点至少修齐 outbound、cli、gateway、adapters、os.md。
4. **`PerceptionConfig`、`DeviceConfig`、`os_bridge` 字段补入 `gateway.md` GatewayConfig 字段表**。

### P1（架构清理）

5. ~~**`team` 上挂 `GatewayState`**~~：模块已删除（2026-06-18）。
6. ~~`eval` 接入 CLI~~：模块已删除。
7. **`heartbeat`**：要么删 doc 中「cron-like 表达式」表述，要么真接 cron 解析（与 `cron` 模块共享 `CronExpression`）。
8. **`tools::ToolContext`**：拆分 `ToolIdentity` / `ToolSandbox` / `ToolModel` 三个子 struct。

### P2（细节）

9. `perception/registry.rs`：评估 `dashmap` / `parking_lot::RwLock` 替换。
10. `providers/preset.rs`：vendor 列表外置 TOML，主代码内只保留 hand-rolled fallback。
11. `agent` heuristics：抽到 `agent/heuristics.rs`，规范中英文关键词。
12. `Snapshot::format_for_prompt`：`### Sensors` 加 top-N 截断防爆 token。
13. ~~删除 `Cargo.toml` 中已 no-op 的 `tui` feature 或在 comment 中明示其语义~~ 已于 2026-06-20 删除该空 feature。

---

## 五、总体评估

* **核心路径成熟**：channels / inbound / agent / outbound / providers / memory / device / perception / tools / acp / plugins / mcp 这一主链已经从单元测试到 E2E 测试完整覆盖，且文档贴近代码。
* **历史包袱已清理**：`flow`、`taskflow`、`server`、`eval` 四大孤岛已于 2026-06-18 全部删除。
* **文档健康度** 大约 85%：核心模块文档准确，外围模块（`outbound`、`adapters`、`heartbeat`、`os.md`）存在与实际代码漂移的局部错误。
* **Prompt 优化空间** 主要在 perception/skills/planner 三处，已有 `enable_summary=false` 的较好默认，下一步是裁掉 `### Sensors` 大 JSON。
* **集成质量**：~~`team` 处于「实现胜过集成」状态~~（已于 2026-06-18 删除,多 agent 协作统一走 ACP）。
