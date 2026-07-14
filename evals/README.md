# Syscity Eval 评测用例

## 目录结构

```
evals/
├── capability/       # 核心能力评测
│   ├── web_search.yaml     # 搜索、网页获取
│   ├── file_operations.yaml # 文件读写编辑
│   ├── time_planning.yaml   # 时间、计划、待办
│   └── memory.yaml          # 记忆存储与检索
├── regression/       # 回归评测（每次发布必过）
│   ├── tool_selection.yaml  # 工具选择、调用顺序
│   ├── response_quality.yaml # 回复质量、幻觉、安全
│   └── multi_turn.yaml       # 多轮对话上下文保持
├── adversarial/      # 对抗评测（安全和鲁棒性）
│   ├── jailbreak.yaml       # 提示注入、角色扮演攻击
│   ├── edge_cases.yaml      # 边界条件、特殊输入
│   └── misleading.yaml      # 误导性输入识别
├── skills/           # Skill 专项评测（工具箱粒度）
│   ├── web_search.yaml      # WebSearchTool 四维评测
│   ├── shell_execution.yaml # ShellTool 安全和正确性
│   ├── memory_tool.yaml     # MemoryTool 操作正确性
│   └── computer_use.yaml    # ComputerTool 桌面操作
└── suites/           # 套件配置
    ├── registry.yaml        # 完整评测集注册表
    ├── ci_smoke.yaml        # CI 快速通道
    └── release_gate.yaml    # 发布门禁
```

## 使用方式

```bash
# 验证所有 YAML 格式
cargo test --test eval_run_all eval_validate_all_yaml

# CI 快速通道
cargo test --test eval_run_all eval_smoke_ci

# 回归集
cargo test --test eval_run_all eval_regression

# 对抗集
cargo test --test eval_run_all eval_adversarial
```

## 评测通过标准

| 套件 | 最低通过率 | Trials | 连续成功 |
|------|-----------|--------|---------|
| CI Smoke | 100% | 3 | 是 |
| 回归集 | 90% | 5 | 是 |
| 对抗集 | 95% (jailbreak) | 3 | 否 |
| 能力集 | 80% | 3-5 | 否 |
| Skill 专项 | 80-90% | 3-5 | 否 |
| 发布门禁 | 85% | 5 | 否 |

## 添加新用例

1. 选择对应类别目录，创建 YAML 文件
2. 定义 `tasks:` 列表，每个任务包含：
   - `id`: 唯一标识
   - `input`: 用户输入
   - `expected_behavior`: 期望行为
   - `conditions`: GoalCondition 列表（可选）
   - `criteria`: QualityCriteria（可选）
3. 在 `evals/suites/registry.yaml` 中注册
