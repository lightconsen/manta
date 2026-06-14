# Eval Module

LLM Evaluation Infrastructure for evaluating agent behavior beyond traditional unit tests.

## Design

Provides rule-based matchers, golden dataset tracking, and LLM-as-a-Judge scaffolding for evaluating agent behavior.

- **Rule-Based Matchers** — Reusable eval rules checked against agent responses
- **Golden Dataset** — Regression testing with expected outputs
- **LLM-as-a-Judge** — Scoring dimensions for model-based evaluation

### Rule-Based Matchers

| Rule | Description |
|------|-------------|
| `MustCallBefore` | A specific tool must be called before another on the same path |
| `NoDuplicateTool` | The same tool with identical args should not be called twice |
| `PathWithinWorkspace` | All file paths in tool calls must be under workspace root |
| `ResponseLength` | Response length must not exceed a token budget |
| `CodeBlockValidity` | All markdown code blocks must be properly closed |

### Golden Dataset

```rust
pub struct GoldenDataset {
    pub cases: Vec<EvalCase>,
    pub version: String,
}

pub struct EvalCase {
    pub id: String,
    pub input: String,
    pub expected_tool_calls: Vec<String>,
    pub expected_output_contains: Vec<String>,
    pub tags: Vec<String>,
}
```

### LLM-as-a-Judge Scoring Dimensions

| Dimension | Description |
|-----------|-------------|
| `Correctness` | Factual accuracy |
| `Helpfulness` | User problem resolution |
| `Safety` | Harmful or inappropriate content |
| `Conciseness` | Brevity without losing information |
| `CodeQuality` | Code correctness and style |

## Key Types

```rust
pub trait EvalRule {
    fn name(&self) -> &str;
    fn check(&self, turn: &AgentTurn) -> RuleResult;
}

pub enum RuleResult {
    Pass,
    Fail(String),
}

pub struct AgentTurn {
    pub user_message: String,
    pub assistant_content: String,
    pub tool_calls: Vec<ToolCallRecord>,
}

pub struct ToolCallRecord {
    pub name: String,
    pub arguments: serde_json::Value,
}

pub enum JudgeDimension {
    Correctness,
    Helpfulness,
    Safety,
    Conciseness,
    CodeQuality,
}
```

## Implemented Features

- Rule-based evaluation framework with composable rules
- Golden dataset for regression testing
- LLM-as-a-Judge scoring with multiple dimensions
- Tool call sequence validation
- Workspace path validation
- Response length budgeting
- Markdown code block validation
- Duplicate tool call detection
- JSON serialization for dataset persistence

