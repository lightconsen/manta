//! Structured action items — translating RCA results into executable tasks.
//!
//! Implements §08: action items with explicit owner, fix, acceptance criteria.
//! Supports four action levels: L0 ReportOnly → L1 CreateTicket → L2 ConfigProposal → L3 AutoFixPR.

use serde::{Deserialize, Serialize};

/// Priority level for action items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Priority {
    /// Critical — blocks release.
    P0,
    /// High — should fix before next release.
    P1,
    /// Medium — nice to have.
    P2,
}

/// Action automation level (§08).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionLevel {
    /// L0: Report only — low confidence, unclear policy.
    #[serde(rename = "report_only")]
    ReportOnly,
    /// L1: Auto-generate ticket with trace, owner, criteria.
    #[serde(rename = "ticket")]
    CreateTicket,
    /// L2: Generate config/prompt diff for human review.
    #[serde(rename = "config_proposal")]
    ConfigProposal,
    /// L3: Auto-fix candidate PR (still needs human review).
    #[serde(rename = "auto_fix_pr")]
    AutoFixPR,
}

/// Impact scope of a problem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactScope {
    /// Associated scenarios / skills affected.
    pub scenarios: Vec<String>,
    /// Number of failed cases.
    pub failure_count: usize,
    /// Risk level description.
    pub risk_level: String,
}

/// A structured, executable action item (§08).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionItem {
    pub id: String,
    /// Business-language problem summary.
    pub problem_summary: String,
    /// Impact scope.
    pub impact_scope: ImpactScope,
    /// Root cause (from RCA).
    pub root_cause: String,
    /// Key evidence (trace IDs, tool calls, critique).
    pub evidence: Vec<String>,
    /// Concrete fix suggestion.
    pub suggested_action: String,
    /// Who should handle this.
    pub owner: String,
    /// How to verify the fix.
    pub acceptance_criteria: String,
    /// Priority.
    pub priority: Priority,
    /// Automation level.
    pub level: ActionLevel,
}

impl ActionItem {
    /// Generate a human-readable summary.
    pub fn summary(&self) -> String {
        format!(
            "[{:?}] {} — {} (owner: {}, action: {:?})",
            self.priority, self.problem_summary, self.suggested_action, self.owner, self.level
        )
    }
}

/// Optimization strategy mapping (§08 table).
pub fn optimization_for_root_cause(root_cause: &str) -> &'static str {
    match root_cause {
        r if r.contains("Prompt") || r.contains("策略") => "修改系统 Prompt、工具选择说明、思考步骤约束、拒答和接管策略",
        r if r.contains("RAG") || r.contains("知识") || r.contains("检索") => "补知识、更新文档、改 chunk、改召回重排、增加引用校验",
        r if r.contains("Tool") || r.contains("工具") || r.contains("API") => "优化工具描述、参数 Schema、错误码、幂等、超时和降级",
        r if r.contains("SOP") || r.contains("流程") || r.contains("业务") => "将 SOP 显式化为状态机、规则、工作流或可校验节点",
        r if r.contains("交互") || r.contains("产品") => "增加澄清入口、确认步骤、人工接管、风险提示",
        r if r.contains("模型") || r.contains("Model") => "换模型、蒸馏/微调、增加 few-shot、拆分复杂任务",
        _ => "需要人工分析确定优化方向",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_item_summary() {
        let item = ActionItem {
            id: "ACT-001".into(),
            problem_summary: "已发货退款场景中未查订单状态".into(),
            impact_scope: ImpactScope {
                scenarios: vec!["售后退款".into()],
                failure_count: 18,
                risk_level: "P0 资损风险".into(),
            },
            root_cause: "退款 Skill 步骤约束缺失".into(),
            evidence: vec!["Trace 显示未调用 order.query".into()],
            suggested_action: "修改退款场景步骤描述".into(),
            owner: "refund_skill_owner".into(),
            acceptance_criteria: "回归集通过率 100%".into(),
            priority: Priority::P0,
            level: ActionLevel::CreateTicket,
        };
        let s = item.summary();
        assert!(s.contains("P0"));
        assert!(s.contains("退款"));
        assert!(s.contains("refund_skill_owner"));
    }

    #[test]
    fn test_optimization_mapping() {
        let fix = optimization_for_root_cause("Tool 选择错误");
        assert!(fix.contains("工具描述"));
    }
}
