# Syscity Agent Harness 架构

> 本页回答：harness 怎么搭、输出品质怎么判/怎么改进、采样覆盖、评估的执行形态、参数与结构性改动的调优边界。
> 状态标注反映 **2026-08-27 当前代码**：✅ 已实现 / ⚠️ 部分实现 / ❌ 缺失。
> 相关文档：`docs/arch.md`（系统总架构）、`docs/eval-status.md`（评测方法论落地）、`docs/reflection.md`（反思引擎）。

---

## 一、什么是 Agent Harness

Harness 不是一组零件，而是把模型与外部世界连接起来的**带反馈的控制回路**。模型负责推理，harness 负责"能动手、看得见、可度量、可干预、可恢复"。

骨架（静态零件）与 harness（闭环系统）的区别，在于最后一条回路——**评估回路**：每一条输出被度量，且度量结果回流改进系统。

```
  零件清单（骨架）                    闭环系统（Harness）
  ───────────────                    ─────────────────
  引擎 · 工具 · 路由 · 记忆          + 工具契约（工具可验证、误用会被拦）
  · 会话 · 上下文                    + 观察回路（每步可回放）
                                    + 状态恢复（崩溃可续）
                                    + 评估回路（每输出可打分、可对比、可回归）
                                    + 优化回路（badcase → 修复 → 回归门禁）
```

---

## 二、整体架构

```
┌────────────────────────────────────────────────────────────────────────────┐
│                    接入层  Channels                                         │
│        wechatmp · mcp · cli · tui · ws · webhook · 人工/批量 eval          │
└────────────────────────────────────┬───────────────────────────────────────┘
                                     │
                                     ▼
┌────────────────────────────────────────────────────────────────────────────┐
│                Agent 核心（编排循环）                                        │
│  engine.rs · agent_engine.rs · prompt_builder · planner · agent_config     │
│  ┌───────────────┬──────────────────┬──────────────────┬───────────────┐   │
│  │ ①工具契约     │ ②观察/回放       │ ③上下文/压缩     │ ④生命周期/恢复│   │
│  │ tools/registry│ transcript       │ compressor       │ lifecycle    │   │
│  │ registrar     │ trace replay     │ budget/disk      │ session_store │   │
│  └───────────────┴──────────────────┴──────────────────┴───────────────┘   │
└────────────────────────────────────┬───────────────────────────────────────┘
                                     │
   ┌──────────────────┬──────────────┼─────────────────┬────────────────────┐
   ▼                  ▼              ▼                 ▼                    ▼
┌────────────┐   ┌────────────┐ ┌──────────────┐  ┌─────────────┐   ┌─────────────┐
│ 工具层 40+ │   │ 记忆        │ │ 模型路由      │  │ 沙箱/安全    │   │ 反思引擎     │
│ shell      │   │ vector+FTS │ │ cost-aware   │  │ rbac        │   │ retrospect   │
│ browser    │   │ dreaming   │ │ fallback     │  │ shell_safety│   │ critic       │
│ file/grep  │   │ session    │ │ circuit-brk  │  │ sandbox     │   │ trajectory   │
│ computer…  │   │            │ │ quota/class  │  │ command_gate│   │              │
└────────────┘   └────────────┘ └──────────────┘  └─────────────┘   └─────────────┘
                                     │
                                     ▼
┌────────────────────────────────────────────────────────────────────────────┐
│               评估回路  Eval Harness（质量判断 + 闭环优化）                  │
│                                                                             │
│  分层评分:  确定性 GoalCondition → LLM Judge → 人工复核                     │
│             ├ 校准/漂移检测   ├ 多 Judge 投票  ├ 统计对比(bootstrap)         │
│             └ badcase 回收 → RCA → action-items → 发布门禁 quality_gate    │
└────────────────────────────────────────────────────────────────────────────┘
```

---

## 三、核心组件清单（含状态）

| 组件 | 作用 | 模块 | 状态 |
|------|------|------|------|
| 引擎 / 编排循环 | 处理入站消息、驱动工具调用 | `src/core/engine.rs`、`src/agent/agent_engine.rs` | ✅ |
| 生命周期管理 | task 创建/取消/优雅关闭、预算控制 | `src/agent/agent_lifecycle.rs`、`budget.rs`、`cost_guard.rs` | ✅ |
| 上下文管理 | 压缩、磁盘预算、防止上下文爆炸 | `src/agent/compressor.rs`、`disk_budget.rs` | ✅ |
| 会话恢复 | 崩溃/重启后恢复会话与请求快照 | `src/agent/session_store/recovery.rs`、`sessions.rs` | ✅ |
| 工具层 | 40+ 工具的注册、契约、校验 | `src/tools/registry.rs`、`registrar.rs`、`validators.rs` | ✅ |
| 模型路由 | 多 provider、成本感知路由、fallback、熔断 | `src/model_router/`（`cost_aware`、`routing`、`health`） | ✅ |
| 记忆 | 向量 + FTS 混合检索、dreaming 归纳 | `src/memory/` | ✅ |
| 反思引擎 | 后台定期回顾轨迹、自我批判、写回记忆 | `src/agent/reflection/` | ✅ |
| 安全沙箱 | RBAC、shell 安全、命令门禁 | `src/tools/rbac.rs`、`shell_safety.rs`、`command_gate.rs` | ✅ |
| Eval Harness | 多 trial 执行 + Wilson CI + 维度平均 | `src/eval/harness.rs` | ✅ |
| 确定性评分 | 退出码/模式/文件/工具调用检查 | `src/goal/condition.rs`、`src/eval/scorer.rs` | ✅ |
| LLM Judge | 6 维度语义评分（temp 0.0、JSON 输出） | `src/agent/reflection/critic.rs` | ✅ |
| 粗筛层 | 风险信号检测（敏感词/过短/工具过多） | `src/eval/scorer.rs`（`RiskSignalChecker`） | ✅ |
| 多 Judge | 多模型独立评分 + 加权聚合 | `src/eval/multi_judge.rs` | ✅ |
| 人工复核 | 低置信/冲突 case 路由给人、复核记录 | `src/eval/human_review.rs` | ✅ |
| 数据集管理 | 评测任务/Golden Set 加载、套件注册表 | `src/eval/dataset.rs`、`loader.rs`、`evals/suites/registry.yaml` | ✅ |
| Judge 校准 | 对标 known-answer cases、漂移检测 | `src/eval/calibration.rs` | ✅ |
| 统计对比 | paired bootstrap，判定 Improved/Regressed | `src/eval/comparison.rs` | ✅ |
| RCA 管线 | badcase 根因诊断 | `src/eval/rca.rs` | ✅ |
| Badcase 回收 | 失败 trial → YAML → 回归套件 | `src/eval/recycle.rs` | ✅ |
| 行动项生成 | 从 RCA 产出可执行的优化建议 | `src/eval/`（`generate_action_items`） | ✅ |
| Skill 专项评分 | 触发/执行/质量/韧性四维 | `src/eval/skill_scorer.rs` | ✅ |
| 发布门禁 | daemon 启动加载 badcase 回归、失败可拒启 | `src/gateway/quality_gate.rs`、`lifecycle.rs` | ✅ |
| 评测套件 | capability / adversarial / regression / calibration / skills | `evals/` 各目录 | ✅ |

### 部分实现

| 组件 | 现状 | 差距 |
|------|------|------|
| 反馈生产线 | 仅 eval 通道可用 | `feedback ops` / `feedback model` 仍为占位 |
| 回归集治理 | `recycle.rs::load_governed_badcase_suite` + `BadcaseGovernance`：过期淘汰（`filter_expired`，默认 90 天）、同输入去重（`is_duplicate`，默认 3 次）、高频任务降级（`effective_pass_rate`），套件加载时实际应用；难度/覆盖标签 + 加权 trial 已实现（`weighted_trials`，standalone 按 `task.trials` 执行） | 治理参数仍为代码默认值，`BadcaseGovernance::from_config` 未接生产加载路径 |
| 上下文压缩 | `compressor.rs` 已实现；`CompressionObservation.retention_ratio` + `quality_flag`（阈值 `min_retention_ratio`）已量化 | eval 门禁侧未校验低保留率（仅记录，未据此失败） |
| 人工复核覆盖 | store + 路由已实现；固定抽样率 `human_review.sampling_rate` 已实现，`score_and_review` 经 `route_case` 按 base_reason ∪ 抽样率路由 | `LayeredScorer` 未接入 harness 生产构造路径（当前仅 eval 测试覆盖） |

---

## 四、四条控制回路（骨架 → harness 的关键）

| 回路 | 检查项 | 状态 |
|------|--------|------|
| ① 工具契约 | 工具 schema 精确、参数有校验、误用会被拦 | ✅ |
| ② 观察/回放 | 每步轨迹可回放、可查因（transcript + trace replay） | ✅ |
| ③ 上下文/压缩 | 长会话不爆上下文、压缩不丢关键信息 | ⚠️ 实现有，效果无量化 |
| ④ 生命周期/恢复 | task 可 spawn/取消/优雅关闭、崩溃可续 | ✅ |
| ⑤ 评估回路 | 每输出可打分、可回归、可门禁 | ✅（离线完整，在线缺失，见 §八） |

---

## 五、采样 / 观测覆盖（评估的数据来源）

评估的前提是每个环节的执行被采样记录。当前观测按 **turn 级** 落盘：每轮对话一个 JSON 文件
（`~/.syscity/turns/…`，`src/observe/`）+ SQLite 指标行，可用 `syscity observe` 聚合；
Trajectory（`src/agent/reflection/trajectory.rs`）从 Turns 构建评分轨迹。

### 已采样的环节

| 环节 | 记录内容 | 模块 |
|------|---------|------|
| 用户消息 / Agent 回复 | `TrajectoryStep::UserMessage` / `AssistantResponse` | `reflection/trajectory.rs` |
| LLM 调用 | `LlmRoundRecord`：provider、model、tokens、耗时、TTFT、finish_reason、error、input/output | `observe/record.rs` |
| 工具调用 | `ObservedToolCall`：name、args、result、success、duration、error（覆盖 memory/planner/shell 等全部工具） | `observe/record.rs`、`agent/turns.rs` |
| token 用量 | 每轮 usage + cache 命中 | `observe/record.rs` |
| 回合元数据 | session、conversation、agent_id、thread、start/finish | `observe/record.rs` |

### 未采样的环节（诊断盲区）

| 环节 | 盲区 |
|------|------|
| 路由决策 | 只记录最终选中的 model；**路由理由、fallback 是否发生、候选链**不落盘 —— 无法区分"答错是换模型导致还是提示词导致" |
| 上下文压缩 | `compressor.rs` 有实现但无观测事件：何时压缩、丢了什么不可见 |
| planner 内部 | 只记到"调用了 planner"，计划 DAG 与步骤状态不落盘 |
| 通道层 | inbound 的 debounce / enrich / route 不进 TurnRecord |
| 生产流量 | ⚠️ 关键：`EvalTaskSource::Online` 仅是数据类型，**真实用户流量未被采样打分**；eval 只跑离线套件 |

**结论：离线评测覆盖"LLM 输出 + 工具行为"这两层（最重要的质量信号），但内部决策层
（路由 / 压缩 / 计划）无一等轨迹，且生产流量完全未采样。** 线上 badcase 若源于路由选错模型，
现有 trace 无法直接归因。

**与调参的耦合：** 参数调优的前提是诊断准确（见 §十），而诊断依赖采样覆盖 —— 路由决策不
采样，就无法判断该不该调路由参数。因此"补路由/压缩轨迹 + 接线上流量采样"比调任何参数更优先。

---

## 六、质量判断：分层评分架构

输出质量是**多维元组**，不是标量，且生成式系统无 ground truth。因此分层组合：

```
第1层  确定性检查  GoalCondition         便宜·可靠·覆盖面窄
         exit_code / grep 模式 / 文件存在 / 调用了某工具 / 未调用某工具
第2层  程序化指标  token · 工具调用次数 · 延迟 · 成本 · 重试 · 连续成功率
第3层  LLM Judge   Critic + multi-judge  覆盖面广·但是不准的仪器(需校准)
         6 维度 · temp 0.0 · 加权聚合 · 校准集对标 · 漂移检测
第4层  人工复核    human_review          最接近 ground truth · 贵(抽样)
```

**三条判断准则**（本仓库已实现的正确做法）：

1. **Judge 是不准的仪器，先校准再用** — `calibration.rs` 对标 known-answer cases，`--drift` 检测漂移；多 judge 一致性应作为 Judge 自身的健康指标。
2. **过程质量 ≠ 结果质量** — 答案对但过程烂（该用工具却 shell 乱抓、该拒绝却硬答）日常看不出来，`trajectory` 打分比 response 打分信息量大。反思引擎（`reflection.rs`）和 `skill_scorer.rs` 已覆盖。
3. **回归比绝对值更有行动价值** — "这周比上周差了吗" 比 "现在 85 分" 有用。`comparison.rs`（bootstrap 判定）已实现，建议设成发布门禁硬门槛。

### 分层评分的执行形态：三处运行位置

上面的分层评分不是只在一处跑。**便宜且确定性的检查放运行时内联，贵且需统计的判定放批量进程**。
严格说，运行时内联的"基础 eval"不叫 eval——统计判定（Wilson CI / bootstrap）需要 N 次配对样本，
单次观测做不了，只能在批量评估中完成。

| 形态 | 位置 | 跑什么 | 现状 |
|------|------|--------|------|
| ① 响应路径内联 | daemon 每 turn 结束钩子 | 确定性 GoalCondition · 程序化指标（token/时长/成本）· `RiskSignalChecker` 风险信号 · 可疑 badcase 标记 | ⚠️ 指标采集与 `cost_guard` 已有；post-turn 钩子缺失，`RiskSignalChecker`（`eval/scorer.rs`）可复用 |
| ② 进程门禁 | 启动时 / cron 周期 | 跑完整 eval harness + 判标 `min_pass_rate` / `require_zero_p0` / `max_degradation` → `Proceed/Rollback/Degrade` | ✅ `gateway/quality_gate.rs`、`lifecycle.rs:358` |
| ③ 离线批量 eval | `syscity eval`（独立进程） | multi-trial + Wilson CI · LLM Judge（Critic / multi-judge）· paired bootstrap 显著性 · RCA / 校准 | ✅ `standalone.rs` + `eval/harness.rs` 等 |

三形态的分工：

```
响应路径（内联·每次）       进程门禁（启动 / cron）       离线批量（eval 命令）
────────────────────────── ─────────────────────────── ──────────────────────────
确定性检查 ✅               完整 harness ✅               multi-trial + Wilson CI ✅
指标采集 ✅                 Proceed / Rollback / Degrade  LLM Judge ✅
风险信号（可复用 ✅）         cron 周期复跑 ✅               bootstrap 显著性 ✅
badcase 标记 ❌缺钩子                                      提议验证（闭环目标）
```

**术语澄清（"离线批量"不是"停机"）：** ①②③ 都可在 daemon 存活期内运行。② 已如此
（`quality_gate` 的 `cron_schedule` 后台触发）；③ 既可用 `syscity eval` 独立进程手工跑，也可由
daemon 的后台批量任务触发（§十二 ⑤⑦ 的自动优化器 / 门禁复跑即为此形态）。"批量"的根因是
**统计判定需要 N 个配对样本**，与"是否停机"无关——唯一不能进每 turn 热路径的就是统计判定本身，
其余（候选生成 / 触发 / 写回 config / hot reload）全都在运行时发生。

**边界纪律：**
- **内联层永不统计判定**——单次观测判不了"改进 / 退化"；判定永远在离线层（`comparison.rs` 需配对样本）。
- **内联层只负责"便宜地发现问题 + 把样本送进回收站"**（`recycle.rs`）。
- 三种形态共享同一套评分逻辑（本 § 四层），差别仅在：跑几次、要不要 LLM Judge、要不要统计。

---

## 七、闭环优化流程（调参 → 数据驱动迭代）

```
生产/评测中失败
     │
     ▼
① badcase-submit / --collect-badcases      ✅ recycle.rs
     │
     ▼
② RCA 根因诊断                             ✅ rca.rs
     │
     ▼
③ action-items 行动项                      ✅ generate_action_items
     │ 按杠杆排序（见 §九）
     ▼
④ 修改: 数据 → 工具 → 提示词 → 路由 → 参数
     │
     ▼
⑤ eval 验证: 多 trial + Wilson CI          ✅ harness.rs
     │
     ▼
⑥ 对比基线: 显著提升? 无回归?              ✅ comparison.rs
     │ 否 ──────────────┐
     ▼ 是                │
⑦ 修复的 badcase 进回归套件                ✅ recycle.rs → evals/badcases/
     │
     ▼
⑧ 发布门禁放行                             ✅ quality_gate
```

> **关键纪律：每次变更（无论指令、工具、路由还是参数）都必须走 ⑤⑥ 两步**——LLM 单次输出方差大，"看效果直接调参"会被噪音骗；多 trial + 置信区间是排除噪音的唯一办法。

---

## 八、已实现 / 缺失汇总

### ✅ 已实现（离线评测闭环完整）

- 多 trial eval harness + Wilson CI + 维度平均 + early stop
- 四层评分（确定性 → 程序化 → LLM Judge → 人工）
- 多 Judge 加权聚合、Judge 校准与漂移检测
- 统计显著性对比（paired bootstrap）
- badcase 回收 → RCA → action-items → 回归套件
- daemon 发布门禁（失败可拒启）
- 反思引擎（后台轨迹自我批判）
- 反馈闭环：`feedback.vote` WS + Web Like/Dislike 按钮（§十二），down 票转 `human:dislike` 待确认 badcase
- 在线质量监控：post-turn RiskSignalChecker 粗筛，高风险命中触发 LLM Judge 深评（`scan_turn_for_badcase`，§八）
- 回归集治理：badcase 难度/覆盖标签 + 加权 trial（`BadcaseGovernance::weighted_trials`，standalone 按 `task.trials` 执行）
- 调优纪律：optimizer/proposer 候选走 harness + `compare_versions` bootstrap verdict，仅 `Improved` 出 patch（`verdict.rs` Gate 1.5，§十二 ⑤⑥）
- 压缩质量量化：`CompressionObservation.retention_ratio` + `quality_flag`（阈值 `min_retention_ratio`，§三）
- 人工复核抽样：`human_review.sampling_rate` 固定抽样，`score_and_review` 经 `route_case` 路由（§三）

### ❌ 缺失（把 harness 从"评测期好用"推向"生产期好用"）

| 缺失项 | 说明 | 建议补法 |
|--------|------|----------|
| 评测看板 | 有基础 eval dashboard（Web），缺趋势/对比可视化 | 基于 eval 产物（pass rate / 维度分 / badcase 聚类 / 对比 verdict）出简单趋势页 |
| Shadow / A-B | 离线 diff 有了（`comparison.rs`），无线上 shadow 分流对比 | 灰度流量按版本分流，用同一判分器离线对比 |

---

## 九、优化杠杆（从高到低，优先动前面的）

| 杠杆 | 为什么比调参强 | 状态 |
|------|---------------|------|
| 1. 数据（badcase → 修复） | 每个 badcase 都是"品质定义的具体化"，修一个是一个 | ✅ 闭环已有 |
| 2. 工具设计 | schema 描述不清/参数歧义是 agent 出错最大来源 | ✅ 工具层，需随 badcase 迭代 |
| 3. 提示词 / 任务说明 | 每条指令、每个 few-shot 直接影响行为 | ✅ `prompt_builder.rs` |
| 4. 模型路由 | 难题换强模型、简单题换便宜模型，影响压倒参数 | ✅ `model_router` cost-aware |
| 5. 检索 / 上下文 | RAG 质量决定信息是否到位 | ✅ `rag/` |
| 6. 后验证 / 重试 | self-verification、工具结果校验 | ⚠️ 部分有 |
| 7. 模型参数（temp 等） | 甜区小、受随机性干扰、必须走 eval 门禁 | ✅ 可调，但最后才动 |

---

## 十、评估后的参数调优（信号 → 参数）

badcase 现象决定动哪个参数（而非凭感觉乱调）。评估后按下列映射选择调整项：

| 评估信号（badcase 现象） | 应调整的参数 | 所在模块 |
|---|---|---|
| 输出过冗长 / 过简 / 风格不对 | `temperature`（默认 0.7）、`max_tokens` | `providers/`、`agent_config.rs` |
| 长会话遗忘 / 上下文溢出 | `max_context_tokens`、`compaction_model`、压缩窗口 | `agent_config.rs` |
| 难题答错 / 简单题浪费算力 | 路由：`default_model` / `preferred_model` / `fallback_model`、cost-aware 阈值 | `model_router/config.rs` |
| 某类任务系统性差 | 任务分类规则 → 路由到更强模型 | `model_router/classifier.rs` |
| 工具选错 / 参数错 | ❌ 不是参数，改工具 schema/描述（最高杠杆） | `tools/` |
| 推理步骤不足 | `max_turns`、`max_concurrent_tools`、或换强模型 | `agent_config.rs` |
| 成本超标 | `budget_limit_usd`、降级模型、`max_tokens` | `model_router/config.rs` |
| 延迟高 | 路由到更快模型、减少无效工具调用 | `model_router` |
| Judge 本身漂了 | `critic_model`、校准集更新 | `reflection/config.rs`、`calibration.rs` |
| 复盘频率 | `interval`、`window_size`、`min_turns` | `reflection/config.rs` |

**两条纪律：**

1. **参数是末位杠杆**（见 §九）：工具设计 > 提示词 > 路由 > 检索 > 参数。badcase 指向工具误用时调 `temperature` 是南辕北辙。
2. **每次调整必须走 §七 闭环的 ⑤⑥ 两步**（eval 多 trial + Wilson CI、对比基线）——LLM 单次输出方差大，凭"看效果"调参会被噪音骗；且诊断依赖采样覆盖（见 §五），盲区未补前，参数调整的依据不可信。

### 怎么改：三个调整入口

| 入口 | 说明 | 典型场景 |
|------|------|---------|
| `config.toml`（`~/.syscity/config.toml`） | 持久配置；`gateway/config.rs` 把 `[default_agent]` 反序列化为 `AgentConfig`，`agent_builder.rs`（`AgentBuilder::config`）/ `Agent::new(AgentConfig)` 用它 spawn agent | 对话行为、模型、成本 |
| CLI 参数（`syscity eval`） | 每次评测运行临时覆盖 | trials / sampling / 被测模型 |
| 代码默认值 + 运行时路由 API | `AgentConfig::default()` 兜底；`model_router/router/admin.rs` 运行时改 provider / fallback 链 | 兜底值、免重启换路由 |

**① config.toml（持久）**：

```toml
model = "claude-3-5-sonnet-20241022"      # 默认模型
model_provider = "anthropic"

[providers]                                # 路由候选池
anthropic = { type = "anthropic", api_key = "$ANTHROPIC_API_KEY" }
openai = { type = "openai", api_key = "$OPENAI_API_KEY" }

[default_agent]
temperature = 0.7          # 输出风格
max_tokens = 2048          # 单次输出上限
max_context_tokens = 16384 # 上下文窗口
max_turns = 20             # 推理轮次上限
# compaction_model = "claude-3-5-haiku-20241022"  # 压缩用便宜模型

[default_agent.reflection_config]
retrospect_enabled = true
critic_model = "claude-3-5-sonnet-20241022" # Judge 模型
[default_agent.reflection_config.retrospect]
interval = 10    # 复盘频率
window_size = 5
min_turns = 3

[cost_guard]
daily_limit_cents = 100   # 每日成本上限
```

**② eval CLI（临时）**：

```bash
syscity eval run ci_smoke --full \
  --trials 5 \
  --sampling-rate 1.0 \
  --provider openai --model gpt-4o
```

套件阈值在 `evals/suites/*.yaml`：`trials`、`min_pass_rate`、`criteria.thresholds`。

**③ 代码默认值 / 运行时**：
- 兜底默认：`AgentConfig::default()` → `temperature: 0.7`、`max_tokens: 2048`、`max_context_tokens: 16384`、`max_turns: None`；`personality.rs::to_agent_config` 同。
- 运行时路由：`model_router/router/admin.rs` 提供增删 provider、改 fallback 链的管理 API（免重启）。

**注意：`temperature` 不是单一旋钮，而是分场景的**：

| 场景 | 值 | 位置 |
|------|-----|------|
| 普通对话 / Agent | 0.7（可配） | `[default_agent] temperature` |
| LLM Judge（Critic） | **写死 0.0** | `critic.rs`（确定性评分，故意不可调） |
| planner 分解 | 0.2 | `planner/decomposer.rs` |
| 多路检索 | 0.7 | `rag/multi_query.rs` |

→ 想调"评估标准"的随机性，调 judge 温度无效；想调对话风格才改 `[default_agent] temperature`。

### 参数归属地图：改哪里（按参数）

badcase 指向某参数时，按此表确定落点。**config.toml 覆盖了大多数"运行行为"参数，但并非全部**：
评测参数、路由深层、分场景温度、算法常量在 config 之外。

| 参数 | config.toml 字段 | 不在 toml 时在哪 | 评估后能调? |
|---|---|---|---|
| `temperature`（对话） | `[default_agent] temperature` | 代码默认 0.7 | ✅ |
| `max_tokens` | `[default_agent] max_tokens` | 代码默认 2048 | ✅ |
| `max_context_tokens` | `[default_agent] max_context_tokens` | 代码默认 16384 | ✅ |
| `max_turns` | `[default_agent] max_turns` | 代码默认 None | ✅ |
| `max_concurrent_tools` | `[default_agent] max_concurrent_tools` | 代码默认 | ✅ |
| `compaction_model` | `[default_agent] compaction_model` | 代码默认 None | ✅ |
| `system_prompt` / `skills_prompt` | `[default_agent]` 对应字段 | personality 生成 | ✅ |
| 默认模型 | 顶层 `model` / `model_provider` | — | ✅ |
| 每 agent 模型 | `agent_models`（agent_id → model） | — | ✅ |
| 每 agent 参数覆盖 | `agent_overrides` | — | ✅ |
| 路由候选池 | `[providers]` | — | ✅ |
| 成本上限 | `[cost_guard] daily_limit_cents` | — | ✅ |
| 发布门禁 | `[quality_gate]`（套件/阈值/拒启） | — | ✅ |
| 观测留存 | `[observe] retention_days` | — | ✅ |
| Judge 模型 `critic_model` | `[default_agent.reflection_config] critic_model` | 代码默认 | ✅ |
| 复盘频率 | `[default_agent.reflection_config.retrospect]` interval/window_size/min_turns | — | ✅ |
| 路由深层（fallback 链 / `circuit_breaker_threshold` / cost-aware 阈值 / `budget_limit_usd`） | ❌ 无 `[model_router]` 节 | `ModelRouterConfig` 代码默认 + `model_router/router/admin.rs` 运行时 API | ⚠️ 运行时 API（免重启） |
| Judge `temperature` | ❌ | `critic.rs` 硬编码 0.0（确定性评分） | ❌ 故意不可调 |
| planner / multi_query `temperature` | ❌ | `planner/decomposer.rs` 0.2、`rag/multi_query.rs` 0.7 | ❌ 代码改动 |
| eval `--trials` / `--sampling-rate` / `--provider` / `--model` | ❌ | CLI `syscity eval run` | ⚠️ 评估时设定，非调优 |
| 套件阈值 | ❌ | `evals/suites/*.yaml`（trials/min_pass_rate/thresholds） | ⚠️ 改 YAML |
| Wilson CI z / bootstrap 次数 / RiskSignal 模式 | ❌ | 代码常量 | ❌ 无需调 |

**读表规则：**
- ✅ = 评估后改 config.toml 即可（正规通道，下一轮 turn 生效，`[hot_reload]` 默认开）
- ⚠️ = 要么是"怎么测"的旋钮（CLI / YAML，调了不提升系统），要么是路由运行时 API（免重启）
- ❌ = 代码层，需 PR + 重编译；Judge 温度是**故意不可调**（保证评分确定性）

**含义：** 若 badcase 指向 ❌ 类参数（如"需要更低 judge 温度"），不要硬塞进 config.toml——它本就不该进；那通常意味着你真正该改的是校准集（`evals/calibration/`）或判分口径，而不是温度。

---

## 十一、结构性改动的动态化（把"结构"变成可搜索的数据）

§十 讲的是**标量参数**（temperature / max_tokens / 预算…），可直接数值调优。但大量优化杠杆是
**离散结构**：工具 schema、prompt 措辞、RAG 切块、流程 SOP——这些"结构"能不能像参数一样动态调整？

**核心原则：把"结构"重制成"数据 + 可测目标"，它就能被搜索。**
结构本身不能调参；但结构一旦写成数据（schema / 文本 / 规则 / 配置），每个候选就是一个数据样本，
套进同一套评分（§六）打分、用同一套比较（`comparison.rs`）判优劣，就变成一次可搜索的迭代。

| 离散结构 | 当前形态 | 能参数化吗 | syscity 落点 |
|---------|---------|-----------|-------------|
| 工具 schema | JSON schema + 描述写死在 `tools/` | ✅ 描述 / 参数说明可拆成多版本候选 | 工具描述作为数据存注册表，版本化 + eval 选优 |
| prompt 措辞 | `system_prompt` / `skills_prompt` 组装 | ✅ 已是 config.toml 数据（§十） | `full_system_prompt()` = base + skills_prompt，每个槽位可有多版本候选 |
| RAG 检索（HyDE / reranker / context_window / multi_query） | `[vector_memory.query_transformer]` / `reranker` / `context_window` / `multi_query` | ✅ 已参数化 | `VectorMemoryConfig`（`gateway/config.rs:363`）真实反序列化 |
| RAG 切块（chunk_size / overlap / separators） | ✅ `[vector_memory.embedding]`（`EmbeddingParams`，`gateway/config.rs`）真实反序列化，`services.rs` 读取 config 值 | ✅ 已参数化 | `config-guide.md:111` 补 `[vector_memory.embedding]` 示例（chunk_size / overlap / strategy） |
| 流程 SOP | goal / standing_orders / planner 分解规则 | ✅ 规则写成数据 + 版本号 | 任务说明作为数据文件，可替换、可对比 |

**统一机制：LLM 提议 + eval 验证 + CI 判定**

- **标量** → 数值搜索：坐标下降 / 网格 / 贝叶斯，候选参数直接跑 eval。
- **结构** → LLM 提议变体：拿 badcase 给 LLM，产出"工具描述改版 / prompt 改版 / SOP 改版"，
  每个变体作为一个数据样本跑 eval；`comparison.rs` 给 verdict（Improved / Regressed / NoSignificantChange）定去留。
- 两者共用同一套 eval harness 与回归门禁，差别只在"候选是怎么产生的"。

**自动化边界（分布图）：**

| 类别 | 自动化程度 | 说明 |
|------|-----------|------|
| 标量参数（temperature / max_tokens / 预算） | 🟢 全自动 | 数值搜索空间有限，eval 自动判定 |
| 结构性改动（工具描述 / prompt / SOP） | 🟢 全自动 | LLM 提议 + eval 验证 + 判定全自动，自动应用不设人批；高险保护靠机械护栏：搜索空间圈定（安全区域不可编辑）+ 自动回滚 + 预算封顶（§十二 护栏） |
| eval 套件本身（判分口径 / 校准集） | 🔴 人工兜底 | 评分标准是"谁定义品质"，全自动会自证正确，需人维护 |

**纪律：** 结构性改动是 §九 的最高杠杆（工具设计 #2、提示词 #3），但每次改动必须走 §七 ⑤⑥
验证（多 trial + Wilson CI）——LLM 提议的变体再合理，没经过 eval 判定就上线等于凭感觉调参。

---

## 十二、全自动闭环：还缺什么（拼图 + 护栏）

把 §七 闭环 + §十一 动态化合起来看，目标系统是"生产跑 → 采样 → 自动提议 → eval 判定 → 自动应用"。
标量可全自动（§十一 🟢）、结构性改动也可全自动（🟢，机械护栏下），但**整条链还缺四块**，按依赖顺序：

| 拼图 | 作用 | 现状 | 缺口 |
|------|------|------|------|
| ① 在线信号接入 | 把生产流量（§五 盲区）送进评估 | ⚠️ `observe/record.rs` 已落盘 turn 级数据，但无打分 | §六 三形态① 的 post-turn 粗筛钩子（复用 `RiskSignalChecker`）+ **人工 Like/Dislike 通道**（见下）+ badcase 标记；且路由/压缩/planner 决策轨迹未采样（§五），自动优化的诊断依据不完整 |
| ② 自动优化器 | 产生候选改动 | ❌ | 标量：数值搜索（坐标下降 / 网格 / 贝叶斯）无现成实现；结构：LLM 提议，需一个后台批量进程（§六 三形态③ 语义，daemon 存活期内由 cron / 事件触发） |
| ③ 自动应用 | 把通过 CI 判定的改动热加载进运行时 | ⚠️ 手动通道已有 | `[hot_reload]` / `config_snapshot()`（`agent_setup.rs:467`）支持配置热更（下一 turn 生效），但"自动写 config.toml + 触发重载"无实现；结构类改动（prompt / 工具描述 / SOP 数据文件）自动落盘无实现 |
| ④ 护栏 | 防止自动改动劣化 / 失控 | ❌ | canary 灰度（先小流量）、回滚（基线快照可回）、预算封顶，见下 |

### 在线信号的具体形态：规则钩子 vs 人工 Like/Dislike（① 的实现）

① 的"post-turn 粗筛"有两种信号源，职责不同，配合使用：

| 维度 | 规则钩子（post-turn 粗筛） | 人工 Like/Dislike |
|------|---------------------------|-------------------|
| 触发方 | 系统自动 | 用户（人） |
| 判断依据 | `RiskSignalChecker` 确定性信号（重试 / 工具报错 / 超时 / 低分） | 用户主观体验（答非所问、不执行指令、冗长…） |
| 捕获盲区 | 只能捕获"机器可测的异常" | 能捕获"机器测不出的不贴合"——两者互补 |
| 代价 | 每轮一次规则扫描，几乎为零 | 每次点击一次用户操作，必须极低摩擦（一个 icon，无弹窗） |
| 产出 | badcase 候选 + 采样标签 | 偏好标签 + badcase 候选 |

**Like/Dislike 通道设计（落地形态）：**

- **传输：** 走 WS RPC 总线，新增 `feedback.vote` 方法（`gateway/ws/core.rs` 的 `match method` 加一个分支，配 `gateway/ws/feedback.rs`）。不新增 HTTP 端点——WS 已是通用 RPC 总线，鉴权在握手层完成，且天然带会话上下文。
- **归属：** `chat.final` 事件必须携带服务端生成的 `turn_id`，前端把 vote 与 turn 绑定，后端落盘到 `TurnRecord` 关联的 feedback store。这样"这条回复当时是什么输入/输出/轨迹"可完整回溯。
- **存储：** 每条 vote 记录 `turn_id + 倾向（like/dislike）+ 可选 comment`。comment 是可选字段，不弹窗强制填，保持一键交互。
- **定位：** 它是 **badcase 捕获的前门 + 自优化的标签源**，不是统计信号——单用户下没有音量去算统计显著性。

**单用户模式下的判断：** syscity 是本地单人系统，"信号源"就是用户本人（永远在线），按钮的价值不在**音量**而在**捕获保真 + 可积累**：把"某次回复我不满意"这种本来会流失的信息，零摩擦地变成一条可回溯、可复现的 badcase 候选；日积月累形成个人偏好标签库。

**使用纪律：**
- **dislike 只做候选标记**，直接进 `human_review` 复核（§六 ④ 层），复核通过才转 badcase → §七 RCA 闭环。绝不把一次 dislike 当 eval verdict 或触发自动改动。
- like 可以作为轻量正样本（进校准集 / 偏好对齐），但不参与回归门禁。
- 按钮只是入口，真正的杠杆在 **badcase 回流 + RCA**（§七），不是按钮本身。

**实现优先级（管道 → 按钮 → 规则钩子）：** 先做共享管道（`turn_id` 归属 + feedback store + `feedback.vote` WS 方法），再做 UI 按钮（`MessageBubble` 的 `AssistantMessageActions` 加两个 icon，near-zero 成本），规则钩子排最后——它依赖前两者把数据接住。

### 护栏规则（自动应用的强制边界）

1. **安全 / 权限 / 成本类改动不设人批，靠机械护栏** — 涉及 RBAC、shell 安全、prompt 注入面的区域**锁死不可编辑**（搜索空间圈定，见 4），成本类改动被 `cost_guard` 预算封顶拦住；候选即使 eval 通过，也只在这些机械护栏内自动生效（§十一 🟢 的具体化）。
2. **canary + 回滚** — 单用户下 canary 的退化形态 = **shadow eval（离线先行）+ 自动回滚**；"小流量灰度"是车队形态（§十三）。`BaselineStore`（`quality_gate.rs`）保留基线快照，回滚由**两种信号触发**：`comparison.rs` 出 Regressed verdict，或在线信号异常（dislike 率 / 风险信号命中率上升）——后者避免热更后实际退化要等到下一次离线 eval 才发现。
3. **eval 套件是护栏本身** — 任何候选（标量或结构）上线前必须过 §七 ⑤⑥（多 trial + Wilson CI + 对比基线），回归套件（`evals/badcases/`）是守门员。**套件样本入口有质量门槛**：dislike / 风险信号命中需经 `human_review` 复核或机械佐证（RiskSignal 同时命中 / 多次踩）才入回归套件，防垃圾样本污染 verdict。
4. **优化器自身的护栏** — 每次自动调优只在**人圈定的搜索空间**内进行（§十一），调优日志全量落盘可审计。
5. **熔断 / 暂停（逃生舱）** — 连续 N 次 Regressed / 成本持续超支 / 在线信号恶化时，**自动暂停自动调优并告警**，人工介入后才恢复；防止"回滚循环"空转（只回滚上一版、不解决根本劣化）。

### 由此可排的最小实现顺序

```
① 共享管道：turn_id 归属 + feedback store + feedback.vote WS 方法
② 人工 Like/Dislike 按钮（badcase 捕获前门，MessageBubble 两个 icon）
③ post-turn 规则钩子（复用 RiskSignalChecker）→ 生产 badcase 入回收站
④ 路由 / 压缩 / planner 决策轨迹采样（补 §五 盲区）
⑤ 标量自动优化器（先圈 3~5 个参数，坐标下降 + 后台批量 eval，cron / 事件触发）
⑥ config 自动写入 + hot reload 触发
⑦ canary / 回滚 + 搜索空间圈定（安全区域锁死，替代人批的机械护栏）
⑧ 结构类 LLM 提议（工具描述 / prompt / SOP 数据化）
```

### 实现路线：五阶段（每阶段独立可验收）

8 步合并成 5 个阶段。顺序原则：**信号先行**（优化器没有输入就是空转）、**便宜的先做**、每阶段可单独上线。

| 阶段 | 内容 | 对应步骤 | 落地文件 | 验收 |
|------|------|---------|---------|------|
| 1a 共享管道 | `turn_id` 归属 + feedback store + `feedback.vote` WS 方法 | ① | `gateway/ws/chat.rs`（`chat.final` 带 `turn_id`）、`gateway/feedback.rs`（新，SQLite `turn_feedback` 表，upsert 幂等）、`gateway/ws/feedback.rs`（新）、`ws/core.rs` 加 `"feedback.vote"` 分支 | WS 冒烟：发 chat → `chat.final` 带 turn_id → `feedback.vote` → store 有行；重复 vote 覆盖的 unit 测试 |
| 1b 人工按钮 | Like/Dislike 按钮（badcase 捕获前门） | ② | `web/src/components/chat/MessageBubble.tsx`（`AssistantMessageActions` 加两个 icon）、`web/src/SyscityWebSocketTransport.ts`（`chat.final` 存 turn_id 到消息状态） | 前端点踩 → `feedback.vote` 落 store |
| 1c 规则钩子 | post-turn 粗筛（复用 `RiskSignalChecker`） | ③ | `core/engine.rs` / `agent_engine.rs`（调 `RiskSignalChecker::scan_turn(&turn_record)`）、`eval/scorer.rs`（补对外 API）、`pending_badcases` 入库（source = `online:risk` / `human:dislike`） | 模拟异常 turn → `pending_badcases` 出现候选 |
| 2 决策轨迹 | 补 §五 盲区（诊断依据） | ④ | `model_router`（`RouteRecord{candidate_chain, chosen, reason, fallback_occurred}`）、`agent/compressor.rs`（压缩观测事件）、planner（DAG + 步状态落盘）、**通道层**（inbound debounce / enrich / route 进 TurnRecord），并入 `observe/record.rs` schema | 触发 fallback / 压缩 / 通道处理 → 轨迹可查、可归因 |
| 3 标量优化器 | 坐标下降 + eval 判定 + 最小自动应用 | ⑤⑥ | `src/eval/optimizer.rs`（新，`ScalarOptimizer`：人圈定搜索空间 + cron/事件触发 + 治理后 badcase 套件跑 harness + `comparison.rs` 对比 `BaselineStore`，仅 Improved 出 patch；后台 tokio 任务注册 `TaskRegistry`）、`apply_patch()`（写回 `config.toml` + hot reload，**走 `base_revision` CAS 防覆盖用户手改**；**调优对象 = 全局 default，per-agent `agent_overrides` 在搜索空间外**）、`config_snapshot()`（`agent_setup.rs:467`）已保证下一 turn 生效 | 圈定 3~5 参数 → 仅显著提升候选产出 patch 并热更；与用户手改冲突时 CAS 拒绝 |
| 4 护栏 | 影子验证 / 回滚 / 预算 / 搜索空间圈定 / 熔断 | ⑦ | `BaselineStore` 快照 + **Regressed 或在线信号异常（dislike / 风险信号上升）触发自动回退**、改成本参数前查 `cost_guard` `daily_limit_cents`、安全相关区域（RBAC / shell 安全 / prompt 注入面）锁死不可编辑（搜索空间圈定）、连续 N 次 Regressed / 超预算 → 自动暂停优化器并告警 | 注入 Regressed 场景 → 自动回退；安全区域候选不产生；连续劣化 → 优化器暂停 |
| 5 结构提议 | 工具描述 / prompt / SOP 改版（🟢 全自动） | ⑧ + 前置改造 | `src/eval/proposer.rs`（新，badcase + 当前变体 → LLM 产 N 候选，候选即数据样本 → harness + verdict）、**前置**：工具描述重制成注册表数据（版本化）、补 `[vector_memory.embedding]` 节参数化 `EmbeddingConfig`（消 `config-guide.md:111` 漂移） | badcase → N 候选 → verdict → 落盘 + hot reload（安全区域在搜索空间外，不产生候选） |

**节奏提醒：** 1a+1b 近零成本、当天见效（点踩 → 可回溯 badcase）；1c、2 是后续一切的地基；3/4 才开始出现"系统自己改标量"；5 最重——必须先做"重制成数据"的前置改造，才有提议器。阶段 2 必须在阶段 3 之前：没轨迹，badcase 归因靠猜，自动优化器的调整依据不可信。

---

## 十三、远期：中心化数据采集与统一优化（联邦优化 · 🔴 先不实现）

> **状态：远期设计，仅记录方向，当前不实现。** 与本地优先 / 单用户前提不同，本节约定 syscity 未来若有大规模用户（opt-in 遥测），如何把 harness 信号采集到后端、聚合、做统一优化。本地单用户时，本节的"车队 / 大规模用户"概念不存在，不适用。

**动机：** §十二 的"全自动闭环"受 N=1 限制——A/B、canary、统计显著性在单个用户上做不了。中心化聚合后，**用户群成为缺失的信号源**：全网样本提供统计功率、badcase 共享产生网络效应、车队灰度让 canary 成立。

### 信号分级（按可采集 / 可聚合 / 优化价值）

| 信号类别 | 采集内容 | 汇总后能做什么优化 | 中心化可行性 |
|---------|---------|-------------------|-------------|
| 人工反馈 | like/dislike、comment、`human_review` 判定结果 | 偏好数据集 → 模型对齐（RLHF 式）/ 提示词调优 / 工具排序；最接近 ground truth | ✅ 低敏可匿名（只有 vote + 判据，无内容） |
| 评测结果 | pass rate、维度分、badcase 聚类、comparison verdict | **badcase 联邦**：一个用户的 badcase（脱敏后）进全球回归套件；车队级回归看板 | ✅ 脱敏后低敏 |
| 程序化指标 | cost、latency、token、重试率、工具成功率 | 全局成本/路由优化、任务普遍性慢/贵检测、模型分级 | ✅ 最低敏 |
| 决策轨迹 | 路由原因、fallback 是否发生、压缩时机、planner DAG | 跨用户验证哪些 fallback / 压缩策略真的有用，归因"调路由还是调提示词" | ⚠️ 不含内容时低敏；含内容需脱敏 |
| 回合/轨迹原始数据 | turn 内容、LLM 输入输出、工具参数/结果 | 离线 eval 语料、失败模式挖掘、benchmark 生成 | ⚠️ 价值最高但隐私成本最大，需 opt-in + 强脱敏 |
| 个人记忆/会话内容 | 个人偏好、长期记忆 | 个人口味不能直接共享；只能聚合为"人群偏好分布" | ❌ 不能原样共享 |

### 大规模用户反馈带来的统一优化（本地做不了的）

1. **badcase 联邦** — 全网回归套件一起进化，网络效应：每个人踩的坑变成大家的护栏；直接填上 §十二 ① 缺的信号源。
2. **车队级 A/B + canary** — 候选 config / prompt / 工具先在用户抽样子集灰度，Wilson CI 用真实 N 算，verdict 统计可信（§十二 ④ canary 在 N=1 下不成立，车队下成立）。
3. **偏好对齐** — 聚合的 like/dislike + 人工复核判据 → 偏好数据集 → 调模型或调 prompt，比单用户偏好更有普适性。
4. **校准集演化** — 全网高置信一致判定喂回 `calibration.rs`，Judge 漂移检测用真实分布。
5. **全局路由/成本优化** — 聚合用量反推哪类任务该用哪个模型、哪些 fallback 值得保留。

### 三条边界（实现前必须满足）

1. **隐私是硬约束** — 轨迹含用户内容（命令输出、文件内容、聊天文本），采集必须 opt-in + 分字段同意 + 脱敏（PII / 工具输出 / 内容只取统计）；它决定了"能采集到什么粒度"。
2. **与本地 / 自托管的矛盾** — 车队只在用户 opt-in 遥测时存在；自托管用户通常最少遥测。要么改中心 SaaS 分发，要么做 opt-in 分享网络（用户决定是否贡献 badcase / 评测结果）——这是分发模型选择，不是纯技术问题。
3. **环境特异 badcase 需归一化** — 用户 badcase 可能依赖其 shell / 文件 / 环境，不能原样聚合；归一化到 **schema / 任务类型层**（"这类任务的工具调用顺序普遍错"而非"某用户的某条命令错了"）。

### 架构形态：数据/指标联邦，优化中心化

比联邦学习更简单——**数据 / 指标联邦，优化中心化**：用户端只上报脱敏数据与评测指标，后端跑优化器（§十二 ⑤ 的扩展），把 config / prompt / badcase 更新分发给用户；**不在用户端训练模型权重**。用户端已有的 `feedback.vote` 管道（§十二 1a）与 `human_review` 判据即是上报入口，中心化只是给它们加一层"可选上报"。

---

## 附录：关键模块索引

```
src/core/engine.rs                 引擎（编排循环）
src/agent/                         生命周期 · 上下文 · 会话 · 反思
src/agent/reflection/              critic / trajectory / retrospect（反思引擎）
src/tools/                         工具注册与实现（40+）
src/model_router/                  路由 · 成本感知 · 熔断 · 配额
src/observe/                       每轮观测（TurnRecord / LLM 轮次 / 工具调用）
src/providers/                     CompletionRequest（temperature / max_tokens / top_p）
src/memory/                        混合检索 · dreaming
src/goal/condition.rs              确定性评分 GoalCondition
src/eval/harness.rs                Eval Harness（多 trial + Wilson CI）
src/eval/scorer.rs                 粗筛 RiskSignal → LLM Judge
src/eval/multi_judge.rs            多 Judge 加权聚合
src/eval/calibration.rs            Judge 校准 + 漂移检测
src/eval/comparison.rs             版本对比（bootstrap 显著性）
src/eval/rca.rs                    badcase 根因诊断
src/eval/recycle.rs                badcase 回收 → 回归套件
src/eval/human_review.rs           人工复核
src/gateway/quality_gate.rs        发布门禁
evals/                             capability / adversarial / regression /
                                   calibration / skills / badcases / suites
```
