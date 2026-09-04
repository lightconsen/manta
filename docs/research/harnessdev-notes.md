# HarnessDev 论文笔记：观点总结 + 对 Syscity 的借鉴

> 来源：HarnessDev（arXiv:2609.01437v1 [cs.SE]，2026-09-01，ByteDance Seed 等）
> 素材：`~/Downloads/HarnessDev.pdf`（pdftotext 全文 2471 行，已全部读完）
> 用途：内部研究笔记。论文与本项目 `docs/harness.md` 的闭环评测设计高度相关。

---

## 一、论文在做什么

把 agent 系统的**评测单元从"任务输出"换成"可运行的基础设施（harness）"**。

- **Harness 的形式化定义**：`H = <E, T, C, S, L, V>`
  - **E**xecution：执行循环、规划、停止条件、调度
  - **T**ools：工具接口、选择、输入输出约束、错误处理
  - **C**ontext：任务/代码/日志/历史/约束如何进入上下文
  - **S**tate：目标、假设、进度、尝试、失败、产物状态
  - **L**ifecycle：前后置 hook、失败/超时处理、恢复、收尾
  - **V**erification：测试、检查、judge、产物校验、轨迹记录
- **两个研究问题**：
  - **RQ1（Creation）**：给一个极弱的可运行种子（只有 CLI/审计输出/无策略原语，**没有**循环、规划器、验证器、状态、重试、停止规则），让模型从零建出完整 harness。
  - **RQ2（Evolution）**：拿已有 harness，靠下游执行的真实反馈迭代修订（10 次成对全量评测预算 + 每轮 2 次 n=5 探针配额）。
- **两种评测设置**：
  - **Self-Eval**（LE=LC）：创建者用自己的 harness 跑任务——衡量"模型–harness 协同设计"。
  - **Unified-Eval**：固定执行器（Gemini 3.1 Pro）跑所有 harness——把 harness 质量与执行器能力**分离归因**。
- **得分与自报隔离**：官方分数只来自真实仓库 diff / 环境最终状态；`result.json` 的 status 必须诚实（success/partial/failed）；隐藏 held-out 任务集全程不开放。

## 二、核心发现（含关键数字）

1. **模型造的 harness 有强有弱**：写作/ML 经验类任务上追平甚至超过人类参考系统；**代码与搜索/研究类仍落后**（Opus Self-Eval 平均 67.8 vs 人类参考 86.2）。
2. **Harness 与模型强耦合，不是可移植资产**：Unified-Eval 下换成固定 Gemini 执行，Opus 的 SWE-Pro 从 69.3 崩到 33.0；重复查询率从 10.1% 飙到 88.2%——为自家模型调的循环/去重策略换个模型就失效。
3. **状态/记忆是全行业最大缺口**：18 个候选里 11 个定义了 State 类，但只有 1 个有 checkpoint；26,679 条轨迹里 **0 个 checkpoint 事件**。
4. **效率与分数脱钩**：同一任务族上 executor token 成本相差约 **19 倍**，且与得分无相关——高分可能极贵。
5. **Evolution 收益不稳定**：64 次官方版本切换里只有 **2 次**有超出噪声的正证据；开发反馈与 held-out 一致率仅 **53.1%**（34/64）；9 个声明的"最终版本"只有 2 个在 held-out 上最优；单次全量评测的成对分数方差约 **±4.75 任务**。
6. **案例研究（附录 F–I）**：
   - **GPT-5.5（Creation）**：14 次读种子后动手，+1251 行；先架构后靠轨迹证据修多对象 JSON 解析、shell 不匹配、重复动作去重、patch 信封。RQ2 里靠 final-review gate（SWE 51→56），最终**选 T2 而非最新的 T7**，因为 T2 是账本上的真 argmax。
   - **Opus（Creation）**：先一次性搭好 1876 行系统（预算/上下文压缩/诚实终止），但两次 settled 得 0 后**都没有做反馈驱动的修改**——暴露"建得好但不闭环"。RQ2 里先分类 189 个失败（诊断优先），再上 diff-grounded self-review（SWE 68→74），最终选 T3；明确拒绝用剩余 7 次预算去追随机峰值（估计噪声 ±3–4 任务）。
7. **反模式**：拿 n=5 探针的聚合分做决策（单任务就摆动 20 分）；对已有机制做外推式扩展而不是定位新的失败桶；往完成判定里硬塞验证缺口注入导致探针回退。

## 三、对 Syscity 的借鉴

> 对照本项目现状：`docs/harness.md`（闭环 harness 设计）、release gate（135 trials、≥0.85）、
> 分层评分（GoalCondition → 程序化指标 → LLM judge → 人工）、badcase 管线、`--only` 失败子集迭代环。

1. **六模块抽象当审计清单用**。`<E, T, C, S, L, V>` 可以直接作为审计 syscity agent 引擎的检查表。对照下来我们的 E/T/C/V/L 都有对应物（引擎循环、工具注册表、上下文压缩、分层验证、生命周期钩子），**S 是行业公认最弱的一格，我们也一样**：论文的 0 checkpoint 事件印证了"状态≠有字段，而是真的被保存/恢复"。可行动项：给 `/goal` 长任务加真正的 checkpoint/恢复（`state.py` 的 save/load/resume 语义），长任务中断后能续跑而不是重来。
2. **得分只看环境终态，不信自报**——我们的确定性优先（GoalCondition 先行）与此一致，是论文验证过的正确方向。保持"验证器读仓库/环境终态而非中间产物文件"；`adapter_status=success 仍可能 score=0` 这一点值得写进 gate 文档，提醒别被"管线跑通了"误导。
3. **引入"固定执行器"对比法**。论文用 Unified-Eval 把"换了执行模型"和"换了 harness"分离归因。对应到我们的迭代环：改引擎/改 base prompt 时，**固定模型与 judge 模型，只变被测变量**（已在做），再进一步——任何"引擎改动让分数涨"的结论都应至少一次在第二个模型上复测，防止 harness–model 耦合带来的假提升（论文的 69.3→33.0 就是教训）。
4. **小样本探针的纪律**。RQ2 的探针是固定 5 题、只给方向性信号、与全量一致率仅 53%。对应我们的 `--only` 失败子集：它也是"方向性信号"，**本地子集 bar 取 ≥85%（比全量需求严格）是对的**；不要拿子集里 ±1 个 trial 的摆动做决策，最终认证必须 CI 全量。
5. **噪声纪律与外部账本**。两个 creator 的最后决策都是"选账本上的最优版本而不是最新版本/随机峰值"，并且都明确估计了单次评测噪声（±3–4 任务 ≈ 我们的 ±4.75）。可行动项：
   - 我们的 paired bootstrap comparison 已覆盖统计显著性，继续保持；
   - **把评测账本落盘**：每次全量 gate 后把 (commit, 分数, 变更, 假设, 结论) 追加进一个文件（如 `evals/ledger.md`）。论文原话："对话记忆撑不过 compaction；文件和 git 历史才是持久层"——这正是本次会话里分析结果被压缩丢失的教训。
6. **诚实状态 + 产物契约**。论文的 `result.json`（honest status）+ `trajectory.jsonl` + `response.md` + 领域终产物 是一套干净的审计契约。我们的 badcase 收集件（critique/response/工具轨迹）可对齐这套字段命名，使 badcase 对第三方（或未来的自动诊断器）可读；"把计划/空产物报成完成"明确记为失败——我们 judge 层已有类似规则，可显式写成契约。
7. **Evolution 的正确姿势：先分类再动刀**。Opus-RQ2 的成功路径是"读 189 个失败 → 分桶（构建/编译/导入错误…）→ 每桶一个结构性修复"；失败路径是"得 0 分后不做反馈驱动修改"（Opus-RQ1）和"外推已有机制"（GPT-RQ2 的 T3）。我们的 fix loop 纪律已对齐（先分类：真行为缺口/judge-infra/trial 噪声），继续坚持**一个失败桶一个根因修复**，不为单次 flaky trial 扭曲 prompt。
8. **防作弊清单进 CI**。论文的 prohibited behavior（禁硬编码任务 ID/答案/期望 patch、禁读 grader、禁篡改评测产物、禁 TODO 占位、禁退化成 one-shot）被做成逐任务审计，违规直接记 0。对应我们的纪律"不降阈值、不改维度凑数"——可以考虑在 CI 加等价检查（例如：gate 相关改动不得同时修改评测 YAML 与通过阈值），把纪律从约定变成机器可执行。
9. **成本轴进 gate 报表**。19 倍成本差与分数无关，提示我们 gate 除了通过率，应同时报 **每任务 token/成本**，让"高分但极贵"的版本显形（对商业化定价也直接有用：专业版/企业版的配额设计需要这个数据）。

## 四、落地追踪（2026-09-04）

| # | 借鉴项 | 状态 |
|---|---|---|
| 1 | S/checkpoint：`/goal` 真正的 save/load/resume | ✅ round 级 checkpoint 硬化（`1005dd0`：原子写 + TaskRegistry 排空）+ **轮内恢复**（轮内消息入 checkpoint，resume 续跑当轮；`restore_threads` 经 spike 判定不是正确接线点） |
| 2 | 得分只看环境终态；`adapter_status=success 仍可能 score=0` | ✅ 写进 `evals/README.md`（迭代纪律 bullet + 产物契约节） |
| 3 | 固定执行器 / 第二模型复测 | ✅ 迭代环 recipe（`388ea3c`） |
| 4 | 小样本探针纪律（`--only` 子集 ≥85% bar） | ✅ 迭代环 recipe（`388ea3c`） |
| 5 | 评测账本落盘 `evals/ledger.md` | ✅ 自动追加（`43b7d25`） |
| 6 | 诚实状态 + 产物契约（字段对齐 result.json / trajectory.jsonl / response.md） | ✅ `evals/README.md` 产物契约节 + judge prompt 显式条款（计划/空产物报成完成 = Fail） |
| 7 | Evolution 先分类再动刀（一个失败桶一个根因修复） | ✅ 迭代环 recipe（`388ea3c`） |
| 8 | 防作弊清单进 CI（硬编码答案 / TODO 占位 / 读 grader 的机器检查） | ⛔ **不做** — `58c9c85` 已机检"阈值/评测 YAML 不可同改"这一最高风险面；judge 层已覆盖编造检测；其余项机器化的边际收益递减 |
| 9 | 成本轴进 gate 报表 | ✅ TrialResult/Suite Summary token 上报（`43b7d25`），goal 侧（`1005dd0`） |

## 五、一句话总结

**HarnessDev 证明了：agent 的分数是"模型 × 基础设施"的乘积，单独评测任何一边都会误判；而基础设施的进化必须靠隔离归因、落盘账本和统计纪律，而不是靠对着小样本聚合分反复采样。** 这正是我们 `docs/harness.md` 闭环设计的方向，论文额外指出了两个补强点：**真·状态/检查点恢复**，和**评测账本落盘**。
