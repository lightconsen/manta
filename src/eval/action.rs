//! Structured action items — translating RCA results into executable tasks.
//!
//! Implements §08: action items with explicit owner, fix, acceptance criteria.
//! Supports four action levels: L0 ReportOnly → L1 CreateTicket → L2
//! ConfigProposal → L3 AutoFixPR.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::eval::rca::{module_to_owner, CandidateModule, RcaResult};
use crate::Result;

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

    /// Build an `ActionItem` from a single RCA result.
    ///
    /// * `index` — sequential index for the action ID.
    /// * `failure_count` — how many trials exhibited this issue.
    pub fn from_rca(rca: &RcaResult, index: usize, failure_count: usize) -> Self {
        let priority = if rca.confidence >= 0.8 {
            Priority::P0
        } else if rca.confidence >= 0.6 {
            Priority::P1
        } else {
            Priority::P2
        };

        let level = if rca.confidence >= 0.85 {
            ActionLevel::ConfigProposal
        } else if rca.confidence >= 0.6 {
            ActionLevel::CreateTicket
        } else {
            ActionLevel::ReportOnly
        };

        let owner = module_to_owner(&rca.responsibility_module);
        let scenarios = vec![format!("{:?}", rca.responsibility_module)];

        ActionItem {
            id: format!("ACT-{:04}", index),
            problem_summary: format!("{}: {}", rca.problem_category, rca.problem_enumeration),
            impact_scope: ImpactScope {
                scenarios,
                failure_count,
                risk_level: format!("{:.0}% confidence", rca.confidence * 100.0),
            },
            root_cause: rca.problem_enumeration.clone(),
            evidence: rca.evidence_chain.clone(),
            suggested_action: optimization_for_root_cause(&rca.problem_enumeration).to_string(),
            owner: owner.to_string(),
            acceptance_criteria: format!("All {} affected scenarios pass", failure_count),
            priority,
            level,
        }
    }
}

/// Generate a deduplicated list of action items from a batch of RCA results.
///
/// Groups by `(problem_enumeration, responsibility_module)`, aggregates failure
/// counts, and takes the highest-confidence entry for priority/level
/// assignment.
pub fn generate_action_items(results: &[RcaResult]) -> Vec<ActionItem> {
    use std::collections::HashMap;

    // Group by (problem_enumeration, responsibility_module)
    let mut groups: HashMap<(&str, &CandidateModule), Vec<&RcaResult>> = HashMap::new();
    for r in results {
        let key = (r.problem_enumeration.as_str(), &r.responsibility_module);
        groups.entry(key).or_default().push(r);
    }

    let mut items: Vec<ActionItem> = groups
        .into_iter()
        .enumerate()
        .map(|(idx, ((_, _), group))| {
            // Highest confidence entry sets priority/level
            let best = group
                .iter()
                .max_by(|a, b| {
                    a.confidence
                        .partial_cmp(&b.confidence)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .copied()
                .unwrap_or_else(|| unreachable!("non-empty group guaranteed by HashMap::entry"));

            let failure_count = group.len();
            ActionItem::from_rca(best, idx, failure_count)
        })
        .collect();

    // Sort by priority (P0 first), then by confidence descending
    items.sort_by(|a, b| {
        let prio_cmp = priority_order(&a.priority).cmp(&priority_order(&b.priority));
        prio_cmp.then_with(|| {
            // Extract confidence from risk_level for sorting
            b.impact_scope.risk_level.cmp(&a.impact_scope.risk_level)
        })
    });

    // Re-assign sequential IDs after sorting
    for (i, item) in items.iter_mut().enumerate() {
        item.id = format!("ACT-{:04}", i + 1);
    }

    items
}

fn priority_order(p: &Priority) -> u8 {
    match p {
        Priority::P0 => 0,
        Priority::P1 => 1,
        Priority::P2 => 2,
    }
}

/// Serialize action items to JSON at `{dir}/actions.json`.
pub fn write_action_items(items: &[ActionItem], dir: &Path) -> Result<std::path::PathBuf> {
    let path = dir.join("actions.json");
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(items)?;
    std::fs::write(&path, &json)?;
    Ok(path)
}

/// Load action items from `{dir}/actions.json`.
///
/// Returns an empty vec if the file does not exist.
pub fn load_action_items(dir: &Path) -> Result<Vec<ActionItem>> {
    let path = dir.join("actions.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let json = std::fs::read_to_string(&path)?;
    let items: Vec<ActionItem> = serde_json::from_str(&json)?;
    Ok(items)
}

pub fn optimization_for_root_cause(root_cause: &str) -> &'static str {
    match root_cause {
        r if r.contains("Prompt") || r.contains("策略") => {
            "修改系统 Prompt、工具选择说明、思考步骤约束、拒答和接管策略"
        }
        r if r.contains("RAG") || r.contains("知识") || r.contains("检索") => {
            "补知识、更新文档、改 chunk、改召回重排、增加引用校验"
        }
        r if r.contains("Tool") || r.contains("工具") || r.contains("API") => {
            "优化工具描述、参数 Schema、错误码、幂等、超时和降级"
        }
        r if r.contains("SOP") || r.contains("流程") || r.contains("业务") => {
            "将 SOP 显式化为状态机、规则、工作流或可校验节点"
        }
        r if r.contains("交互") || r.contains("产品") => {
            "增加澄清入口、确认步骤、人工接管、风险提示"
        }
        r if r.contains("模型") || r.contains("Model") => {
            "换模型、蒸馏/微调、增加 few-shot、拆分复杂任务"
        }
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

    #[test]
    fn test_from_rca_priority_mapping() {
        let rca = make_test_rca(0.85);
        let item = ActionItem::from_rca(&rca, 1, 3);
        assert_eq!(item.priority, Priority::P0);
        assert_eq!(item.level, ActionLevel::ConfigProposal);
        assert_eq!(item.id, "ACT-0001");

        let rca_p1 = make_test_rca(0.7);
        let item_p1 = ActionItem::from_rca(&rca_p1, 2, 1);
        assert_eq!(item_p1.priority, Priority::P1);
        assert_eq!(item_p1.level, ActionLevel::CreateTicket);

        let rca_p2 = make_test_rca(0.4);
        let item_p2 = ActionItem::from_rca(&rca_p2, 3, 1);
        assert_eq!(item_p2.priority, Priority::P2);
        assert_eq!(item_p2.level, ActionLevel::ReportOnly);
    }

    #[test]
    fn test_from_rca_field_mapping() {
        let mut rca = make_test_rca(0.9);
        rca.problem_category = "Tool Selection".into();
        rca.problem_enumeration = "关键工具未调用".into();
        rca.evidence_chain = vec!["trace-1".into(), "trace-2".into()];
        rca.responsibility_module = CandidateModule::ToolSelection;

        let item = ActionItem::from_rca(&rca, 5, 10);
        assert!(item.problem_summary.contains("Tool Selection"));
        assert!(item.problem_summary.contains("关键工具未调用"));
        assert_eq!(item.root_cause, "关键工具未调用");
        assert_eq!(item.evidence, vec!["trace-1", "trace-2"]);
        assert_eq!(item.owner, "tool_team");
        assert_eq!(item.impact_scope.failure_count, 10);
    }

    #[test]
    fn test_generate_action_items_dedup() {
        let results = vec![
            make_test_rca(0.9), // group A
            make_test_rca(0.9), // same group (default uses ToolSelection + empty "")
            make_test_rca_with(0.7, "问题B", CandidateModule::ContextMemory),
            make_test_rca_with(0.5, "问题B", CandidateModule::ContextMemory),
        ];

        let items = generate_action_items(&results);
        // Two unique groups -> two items
        assert_eq!(items.len(), 2, "Should dedup to 2 items");

        let memory_items: Vec<_> = items.iter().filter(|i| i.owner == "memory_team").collect();
        assert_eq!(memory_items.len(), 1);
        // Highest confidence in group B is 0.7 -> P1
        assert_eq!(memory_items[0].priority, Priority::P1);
        // failure_count = number of results in that group
        assert_eq!(memory_items[0].impact_scope.failure_count, 2);
    }

    #[test]
    fn test_write_load_roundtrip() -> Result<()> {
        use tempfile::TempDir;

        let items = vec![
            ActionItem::from_rca(&make_test_rca(0.9), 1, 3),
            ActionItem::from_rca(&make_test_rca(0.6), 2, 1),
        ];

        let dir = TempDir::new()?;
        let path = write_action_items(&items, dir.path())?;
        assert!(path.exists());

        let loaded = load_action_items(dir.path())?;
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, items[0].id);
        assert_eq!(loaded[0].priority, items[0].priority);
        assert_eq!(loaded[1].owner, items[1].owner);

        Ok(())
    }

    #[test]
    fn test_load_action_items_missing_file() -> Result<()> {
        use tempfile::TempDir;

        let dir = TempDir::new()?;
        let items = load_action_items(dir.path())?;
        assert!(items.is_empty());
        Ok(())
    }

    /// Helper — create an RcaResult with given confidence and defaults.
    fn make_test_rca(confidence: f64) -> RcaResult {
        RcaResult {
            phenomenon: "non_responsive".into(),
            process_deviation: "step1".into(),
            responsibility: "tool_team".into(),
            problem_category: "".into(),
            problem_enumeration: "".into(),
            responsibility_module: CandidateModule::ToolSelection,
            sub_responsibility: None,
            evidence_chain: vec!["trace-evidence".into()],
            fix_suggestion: "fix it".into(),
            confidence,
            analysis_duration_ms: 100,
            entry: crate::eval::rca::BadcaseEntry::AutoDetected,
            completed_at: std::time::SystemTime::now(),
        }
    }

    /// Helper — create an RcaResult with custom problem/module.
    fn make_test_rca_with(confidence: f64, problem: &str, module: CandidateModule) -> RcaResult {
        RcaResult {
            phenomenon: "non_responsive".into(),
            process_deviation: "step1".into(),
            responsibility: format!("{:?}", module),
            problem_category: "".into(),
            problem_enumeration: problem.into(),
            responsibility_module: module,
            sub_responsibility: None,
            evidence_chain: vec!["trace-evidence".into()],
            fix_suggestion: "fix it".into(),
            confidence,
            analysis_duration_ms: 100,
            entry: crate::eval::rca::BadcaseEntry::AutoDetected,
            completed_at: std::time::SystemTime::now(),
        }
    }
}
