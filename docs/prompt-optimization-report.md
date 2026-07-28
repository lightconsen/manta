# Prompt Optimization Report

> 汇总系统中所有 Prompt 构建位置，分析合理性，并给出优化建议。
> 生成日期: 2026-07-28

---

## 目录

1. [Prompt 构建位置汇总](#1-prompt-构建位置汇总)
2. [质量分析](#2-质量分析)
3. [优化建议](#3-优化建议)
4. [优先级矩阵](#4-优先级矩阵)

---

## 1. Prompt 构建位置汇总

### A. 核心 Agent 系统 Prompt

| # | 位置 | 内容 | 语言 | 类型 |
|---|------|------|------|------|
| A1 | `src/agent/mod.rs:396-442` | `AgentConfig::default()` — 默认 system prompt (~600 words)，包含身份、工具规则、响应格式、Current Time | English | 硬编码常量 |
| A2 | `src/agent/mod.rs:464-468` | `full_system_prompt()` — 将 skills_prompt 追加到 system prompt 后 | English | 动态拼接 |
| A3 | `src/agent/mod.rs:492-550` | `full_system_prompt_with_personality()` — 合并 base prompt + SOUL.md + personality memory + host env | English | 动态拼接 |
| A4 | `src/agent/mod.rs:1198-1228` | 技能注入 — 根据用户消息 prefilterskills 动态注入匹配的技能 | English | 动态拼接 |
| A5 | `src/agent/mod.rs:1174-1195` | Vector memory 上下文注入 — `format_for_injection()` | English | 动态拼接 |
| A6 | `src/agent/personality.rs:380-423` | `build_system_prompt()` — 7段 personality 组装: Bootstrap > Identity > Soul > Agents > Tools > Heartbeat > Memory | English | 动态拼接 |
| A7 | `src/agent/personality.rs:431-463` | `build_subagent_prompt()` — 精简版 (排除 Bootstrap, Heartbeat, Memory) | English | 动态拼接 |
| A8 | `src/agent/personality.rs:352-359` | Agent Identity 注入 — agent_id + directory | English | 动态拼接 |
| A9 | `src/agent/mod.rs:547-549` | Host Environment 注入 — 动态 OS/Desktop 信息 | English | 动态拼接 |
| A10 | `src/agent/prompt_builder.rs:483-513` | `build_from_context()` — 动态 prompt builder: task type/phase/priority/token budget | English | 动态构建 |

### B. 辅助/内部 Prompts

| # | 位置 | 内容 | 语言 | 类型 |
|---|------|------|------|------|
| B1 | `src/goal/plan.rs:12-28` | `GOAL_PARSE_SYSTEM_PROMPT` — 解析用户 goal 为 JSON 条件数组 | English | 硬编码常量 |
| B2 | `src/goal/runner.rs:369-398` | `build_agent_system_prompt()` — goal 执行子 agent 的 system prompt | **Chinese** | 动态拼接 |
| B3 | `src/planner/decomposer.rs:190-267` | `DECOMPOSITION_SYSTEM_PROMPT` — DAG 子任务分解 (~80行，包含 ~20 个硬编码 DesktopAction 类型) | English | 硬编码常量 |
| B4 | `src/agent/reflection/critic.rs:17-44` | `TRAJECTORY_CRITIC_PROMPT` — 轨迹评估: 6 个标准 + JSON schema | English | 硬编码常量 |
| B5 | `src/agent/compaction.rs:18-28` | `DEFAULT_MEMORY_FLUSH_PROMPT` — memory flush 用户 prompt | English | 硬编码常量 |
| B6 | `src/agent/compaction.rs:31-37` | `DEFAULT_MEMORY_FLUSH_SYSTEM_PROMPT` — memory flush 系统 prompt | English | 硬编码常量 |
| B7 | `src/gateway/ws.rs:840-850` | Session 标题生成 — "最多6个词的摘要" | English | 动态格式化 |
| B8 | `src/memory/query.rs:48-68` | HyDE prompt — 给定 query 生成虚构答案用于检索 | English | 动态格式化 |
| B9 | `src/memory/manager.rs:966-992` | LLM compaction prompt — 对话提取关键事实为 JSON | English | 动态格式化 |

### C. 评估/诊断 Prompts

| # | 位置 | 内容 | 语言 | 类型 |
|---|------|------|------|------|
| C1 | `src/eval/standalone.rs:157-164` | 工具使用指南追加 — 偏好专用工具 > shell | English | 硬编码拼接 |
| C2 | `src/eval/agent_type.rs:68-106` | `scoring_emphasis()` — 每种 AgentType 的评估关注点指导 (6 种类型) | English | 静态方法 |
| C3 | `src/eval/rca.rs:619-628` | RCA 模块诊断 prompt — 评估回复是否忠实于证据 | English/Chinese混 | 动态格式化 |
| C4 | `src/planner/error_diagnosis.rs:420-444` | 错误诊断 prompt — 分析错误输出 JSON (category/severity/remediation) | English | 动态格式化 |
| C5 | `src/planner/tool_chain.rs:216-241` | 前置条件分析 prompt — 分析 goal 需要的先决条件 | English | 动态格式化 |

---

## 2. 质量分析

### 2.1 优点

1. **Personality 分层合理** — `personality.rs` 的 7 段组装 (Bootstrap > Identity > Soul > Agents > Tools > Heartbeat > Memory) 优先级清晰，sub-agent 自动排除无关段

2. **工具描述自动生成** — Tool trait 的 `description()` + `parameters_schema()` 由 rust 代码维护，减少 prompt 与实际工具签名之间的 drift

3. **动态上下文注入** — memory context、skills、host environment 都在运行时根据实际情况注入，避免静态 prompt 携带过多无用信息

4. **Sub-agent prompt 精简设计** — `build_subagent_prompt()` 自觉排除 Bootstrap/Heartbeat/Memory，减少 token 浪费

5. **Critic prompt 结构清晰** — 6 个评估维度 + JSON schema 示例 + observation 要求，格式统一

### 2.2 问题

#### P0 — 必须修复

1. **中英文混杂 (B2, C3)**
   - `src/goal/runner.rs:369-398` 是**中文** prompt，其它所有 prompt 都是英文
   - `src/eval/rca.rs:619-628` prompt 是英文但 verdict 是中文 `"回复未忠实于工具执行结果"`
   - 混合语言可能导致 LLM 输出语言不一致

2. **"Current Time" 是 dead promise (A1:441-442)**
   - 默认 prompt 声称 "The current time is provided in the context"，但代码中没有找到任何地方实际注入当前时间
   - 这会导致 LLM 产生困惑时间感知的幻觉——它以为有时间但实际上没有

3. **DECOMPOSITION_SYSTEM_PROMPT 过度硬编码 (B3)**
   - `planner/decomposer.rs:190-267` 包含约 80 行代码，硬编码了 ~20 种 DesktopAction 类型和 ~7 种 verification 类型
   - 如果新增 DesktopAction，需要同步更新这里的 prompt
   - prompt 中还包含了 device_oscilloscope 等具体示例，与 decomposer 的通用定位不符

#### P1 — 应该优化

4. **默认 system prompt 过长且无结构 (A1)**
   - ~600 字的纯文本，没有分层优先级，Tool Usage Rules 和 Response Formatting Guidelines 混在一起
   - Tool Usage Rules 中的具体工具名称 (cron、浏览器等) 应该动态生成而不是硬编码

5. **PromptBuilder 调用时机太晚 (A10 vs A1-A9)**
   - `build_from_context()` 在 agent 中调用时，personality/memory/skills 已经在 `full_system_prompt_with_personality()` 中注入了
   - 导致 PromptBuilder 的 task type 感知、phase 感知、priority 剪枝等功能效果有限——它只能修改已经构建好的完整 prompt 的外层

6. **`detect_task_type()` 使用关键词匹配**
   - `prompt_builder.rs` 的任务类型检测基于简单关键词匹配 (如包含 "bug" 认为是 Debugging)
   - 应该使用 embedding 分类或交给 LLM 判断

#### P2 — 值得改进

7. **Session title prompt 硬编码字符串 (B7)**
   - `gateway/ws.rs:840-850` 的 "Summarize...at most 6 words" prompt 直接在函数体字符串内
   - 不利于统一管理和测试

8. **错误诊断 prompt 没有 system message (C4)**
   - `error_diagnosis.rs:435` 只传了 `Message::user(prompt)`，没有 system message 约束行为
   - 相比 critic prompt 有完整的 system prompt + JSON schema，这里的 prompt 完整性较低

9. **Tool chain 分析 prompt 缺少类型定义联动 (C5)**
   - `action_type` 的枚举值 (`launch_app`, `browse_files`, `list_processes`, `tcp_connect`) 硬编码在 prompt 中，没有与 Rust 类型同步

10. **HyDE prompt 缺少格式约束 (B8)**
    - 只有一句话描述，没有 negative example 或格式示例来控制输出质量

---

## 3. 优化建议

### P0 — 立即执行

#### R1: 统一 promt 语言

**涉及文件:**
- `src/goal/runner.rs:369-398`
- `src/eval/rca.rs:619-628`

**操作:** 将 B2 和 C3 的中文改为英文，保持全系统一致性。

**B2 改动示例 (`goal/runner.rs`):**
```rust
fn build_agent_system_prompt(&self) -> String {
    format!(
        r#"You are an autonomous goal-execution agent. Your task is to complete the following goal.

## Goal
{}

## Check Conditions (all must pass)
{}

## Rules
1. Use the available tools to accomplish the goal.
2. After each tool call, the LLM will receive the tool result.
3. Conditions are checked automatically after each round — you only need to take action.
4. You may call tools multiple times to iterate.
5. Working directory: {}
6. Do not modify .git directories or sensitive configuration files.
7. When done, reply with a brief completion message."#,
        ...
    )
}
```

---

#### R2: 修复 "Current Time" dead promise

**涉及文件:** `src/agent/mod.rs:396-442`

**操作:** 两种选择:
- **方案 A (推荐):** 在 `full_system_prompt_with_personality()` 或 `build_prompt_context()` 中注入当前时间
- **方案 B:** 删除 prompt 中的 "Current Time" 段落

**方案 A 示例 (`agent/mod.rs` 在 base_prompt 拼接后):**
```rust
let time_info = format!(
    "\n\n## Current Time\n{}",
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z")
);
result = format!("{}{}", result, time_info);
```

---

#### R3: 精简 DECOMPOSITION_SYSTEM_PROMPT

**涉及文件:** `src/planner/decomposer.rs:190-267`

**操作:**
- 将 DesktopAction 类型的完整列表从 prompt 移到代码注释或类型定义中
- prompt 只保留核心规则和少量示例
- 考虑自动从 DesktopAction enum 生成类型描述

**预估效果:** 减少约 50 行 prompt (从 ~80 行减到 ~30 行)，降低维护成本。

---

### P1 — 下次迭代

#### R4: 结构化默认 system prompt

**涉及文件:** `src/agent/mod.rs:396-442`

**操作:** 将纯文本 prompt 改为分层结构，使用 XML-like tags 或 markdown heading 分层:
- `## Identity & Purpose` (简短)
- `## Behavioral Rules` (工具使用约束)
- `## Response Format` (格式化要求)
- `## Context Sources` (时间、memory 等)

另外将具体工具名（cron、浏览器等）改为动态注入。

---

#### R5: 提前 PromptBuilder 调用时机

**涉及文件:** `src/agent/mod.rs` + `src/agent/prompt_builder.rs`

**操作:** PromptBuilder 应该在 `full_system_prompt_with_personality()` 之前调用，让 builder 的 task type 和 phase 感知能够影响哪些 personality 段被包含。

---

#### R6: 提升任务类型检测

**涉及文件:** `src/agent/prompt_builder.rs` 的 `detect_task_type()`

**操作:** 将关键词匹配升级为:
1. 快速方案: 增加更多关键词并引入否定规则
2. 长期方案: 使用 embedding 相似度分类

---

### P2 — 后续优化

#### R7: 提取 session title prompt 为常量

**涉及文件:** `src/gateway/ws.rs:840-850`

**操作:** 定义为 `const SESSION_TITLE_PROMPT: &str` 并移到模块顶部或独立的 prompt 常量区。

---

#### R8: 为 error diagnosis 添加 system message

**涉及文件:** `src/planner/error_diagnosis.rs:420-444`

**操作:** 添加约束行为的 system message:
```rust
messages: vec![
    Message::system("You are a root cause analysis engine. Output only valid JSON."),
    Message::user(prompt),
],
```

---

#### R9: 联动类型定义生成 action_type 枚举

**涉及文件:** `src/planner/tool_chain.rs:216-241`

**操作:** 从 CheckActionType enum 自动推导允许的 action_type 列表，或至少添加注释指向类型定义位置。

---

#### R10: 增强 HyDE prompt

**涉及文件:** `src/memory/query.rs:48-68`

**操作:** 添加格式控制和负例:
```rust
const HYDE_SYSTEM_PROMPT: &str = r#"You are a query expansion assistant. Given a search query,
write a short factual paragraph answering it. Rules:
- No meta-commentary, no prefacing ("Based on...", "I think...")
- Write ONLY the answer paragraph
- If the query is ambiguous, cover the most likely interpretation"#;
```

---

## 4. 优先级矩阵

| 优先级 | 建议 | 影响 | 工作量 | 风险 |
|--------|------|------|--------|------|
| **P0** | R1: 统一语言 | LLM 输出语言一致性 | ~5 min | 低 |
| **P0** | R2: 修复 Current Time | 消除时间幻觉 | ~10 min | 低 |
| **P0** | R3: 精简 decomposer | 维护成本 | ~15 min | 中 (需验证示例正确性) |
| **P1** | R4: 结构化默认 prompt | token 效率、可控性 | ~20 min | 中 (需测试回归) |
| **P1** | R5: 提前 PromptBuilder | prompt 动态性真正生效 | ~30 min | 中 (影响 agent 主流程) |
| **P1** | R6: 任务类型检测升级 | 分类准确率 | ~1h | 低 |
| **P2** | R7-R10 | 代码质量 | ~15 min each | 低 |

---

## 附录 A: 配置文件中的 Prompt

配置文件 (`config.toml` / agent YAML) 中也可配置 prompt 相关字段:

| 字段 | 位置 | 用途 |
|------|------|------|
| `system_prompt` | agent config | 替换默认 system prompt |
| `personality.soul` | agent dir SOUL.md | 核心人格 |
| `personality.identity` | agent dir IDENTITY.md | 身份定义 |
| `personality.heartbeat` | agent dir HEARTBEAT.md | 周期性任务 |

这些文件路径上的 prompt 属于用户可控范围，不在本次优化范围内。

---

## 附录 B: Prompt 总量估算

| 来源 | 估算 tokens | 占比 |
|------|------------|------|
| 默认 system prompt (A1) | ~800 | 25% |
| Personality 组装 (A6-A9) | ~500-2000 | 15-40% |
| Skills (A4) | ~500-3000 | 15-50% |
| Memory context (A5) | ~500-2000 | 15-40% |
| Host Environment (A9) | ~100 | 3% |
| **总 system prompt** | **~2000-8000** | **100%** |

> 注: 实际 token 消耗因 agent 配置和 memory 量而异。建议添加 `--debug-prompt` 标志打印完整 prompt 以辅助调优。
