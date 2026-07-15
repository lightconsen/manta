//! Skill Scorer — evaluates `SkillEvalDesign` against trial execution data.
//!
//! Implements the four-dimensional skill evaluation from §02 / §06-6:
//! 1. **Trigger** — did the agent call (or not call) the expected tool?
//! 2. **Execution** — required tools present? forbidden tools absent? params correct?
//! 3. **Quality** — does the response meet must_contain / must_not_contain / min_length?
//! 4. **Resilience** — retroactive detection of tool failures and degradation checks.
//!
//! This runs alongside GoalCondition (Code Scorer) and Critic (LLM Judge)
//! as an additional scoring layer within `EvalHarness`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::eval::dataset::{
    DegradeExpectation, ExecutionCase, FailureMode, ParamMatcher, QualityCase,
    ResilienceCase, SkillEvalDesign, TriggerCase,
};
use crate::eval::harness::ToolCallSummary;

// ── Sub-results ─────────────────────────────────────────────────────────

/// Result of evaluating a single trigger case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerCheckResult {
    pub case_label: String,
    pub passed: bool,
    pub detail: String,
}

/// Result of evaluating a single execution case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionCheckResult {
    pub scenario: String,
    pub passed: bool,
    pub detail: String,
}

/// Result of evaluating a single quality case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCheckResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

/// Result of evaluating a single resilience case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResilienceCheckResult {
    pub passed: bool,
    pub detail: String,
}

/// Aggregated result from evaluating a complete `SkillEvalDesign`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCheckResult {
    pub trigger_results: Vec<TriggerCheckResult>,
    pub execution_results: Vec<ExecutionCheckResult>,
    pub quality_results: Vec<QualityCheckResult>,
    pub resilience_results: Vec<ResilienceCheckResult>,
    /// True iff **all** sub-results passed.
    pub passed: bool,
}

impl SkillCheckResult {
    fn new() -> Self {
        Self {
            trigger_results: Vec::new(),
            execution_results: Vec::new(),
            quality_results: Vec::new(),
            resilience_results: Vec::new(),
            passed: true,
        }
    }

    fn compute_passed(&mut self) {
        self.passed = self.trigger_results.iter().all(|r| r.passed)
            && self.execution_results.iter().all(|r| r.passed)
            && self.quality_results.iter().all(|r| r.passed)
            && self.resilience_results.iter().all(|r| r.passed);
    }
}

// ── Skill Scorer ────────────────────────────────────────────────────────

/// Stateless scorer for `SkillEvalDesign` checks.
pub struct SkillScorer;

impl SkillScorer {
    /// Evaluate a `SkillEvalDesign` against trial execution data.
    ///
    /// # Parameters
    /// * `design` — the four-dimensional skill test design.
    /// * `tool_calls` — all tool calls made during this trial.
    /// * `response` — the agent's final text response.
    pub async fn evaluate(
        design: &SkillEvalDesign,
        tool_calls: &[ToolCallSummary],
        response: &str,
    ) -> SkillCheckResult {
        let mut result = SkillCheckResult::new();

        // 1. Trigger checks
        for tc in &design.trigger {
            let r = Self::evaluate_trigger(tc, tool_calls);
            result.trigger_results.push(r);
        }

        // 2. Execution checks
        for ec in &design.execution {
            let r = Self::evaluate_execution(ec, tool_calls);
            result.execution_results.push(r);
        }

        // 3. Quality checks
        for qc in &design.quality {
            let r = Self::evaluate_quality(qc, response);
            result.quality_results.push(r);
        }

        // 4. Resilience checks (retroactive — detect failures from tool call results)
        for rc in &design.resilience {
            let r = Self::evaluate_resilience(rc, tool_calls, response);
            result.resilience_results.push(r);
        }

        result.compute_passed();
        result
    }

    // ── Trigger ──────────────────────────────────────────────────────

    fn evaluate_trigger(
        tc: &TriggerCase,
        tool_calls: &[ToolCallSummary],
    ) -> TriggerCheckResult {
        if let Some(ref st) = tc.should_trigger {
            let found = tool_calls.iter().any(|t| t.name == st.expect_tool);
            let label = format!("should_trigger({})", st.expect_tool);
            if found {
                TriggerCheckResult {
                    case_label: label,
                    passed: true,
                    detail: format!("Tool '{}' was called as expected", st.expect_tool),
                }
            } else {
                let called_names: Vec<&str> = tool_calls.iter().map(|t| t.name.as_str()).collect();
                TriggerCheckResult {
                    case_label: label,
                    passed: false,
                    detail: format!(
                        "Expected tool '{}' was not called. Tools called: {:?}",
                        st.expect_tool, called_names
                    ),
                }
            }
        } else if let Some(ref nt) = tc.should_not_trigger {
            if nt.expect_no_tool.is_empty() {
                // Means no tool should be called at all
                let passed = tool_calls.is_empty();
                TriggerCheckResult {
                    case_label: "should_not_trigger(any)".into(),
                    passed,
                    detail: if passed {
                        "No tools were called as expected".into()
                    } else {
                        format!(
                            "Expected no tool calls but found: {:?}",
                            tool_calls.iter().map(|t| &t.name).collect::<Vec<_>>()
                        )
                    },
                }
            } else {
                let found = tool_calls.iter().any(|t| t.name == nt.expect_no_tool);
                let passed = !found;
                TriggerCheckResult {
                    case_label: format!("should_not_trigger({})", nt.expect_no_tool),
                    passed,
                    detail: if passed {
                        format!("Tool '{}' was correctly not called", nt.expect_no_tool)
                    } else {
                        format!(
                            "Tool '{}' was called but should not have been",
                            nt.expect_no_tool
                        )
                    },
                }
            }
        } else {
            // Empty trigger case — vacuously pass
            TriggerCheckResult {
                case_label: "empty_trigger_case".into(),
                passed: true,
                detail: "No trigger conditions defined".into(),
            }
        }
    }

    // ── Execution ────────────────────────────────────────────────────

    fn evaluate_execution(
        ec: &ExecutionCase,
        tool_calls: &[ToolCallSummary],
    ) -> ExecutionCheckResult {
        let mut details = Vec::new();
        let mut all_pass = true;

        // Required tools
        for required in &ec.required_tools {
            let found = tool_calls.iter().any(|t| t.name == *required);
            if !found {
                all_pass = false;
                details.push(format!("Required tool '{}' was not called", required));
            }
        }
        if !ec.required_tools.is_empty() && details.is_empty() {
            details.push("All required tools were called".into());
        }

        // Forbidden tools
        for forbidden in &ec.forbidden_tools {
            let found = tool_calls.iter().any(|t| t.name == *forbidden);
            if found {
                all_pass = false;
                details.push(format!("Forbidden tool '{}' was called", forbidden));
            }
        }
        if !ec.forbidden_tools.is_empty() {
            // Only add "no forbidden tools called" if we didn't already detect one
            let has_forbidden_fail = ec.forbidden_tools.iter().any(|f| {
                tool_calls.iter().any(|t| t.name == *f)
            });
            if !has_forbidden_fail {
                details.push("No forbidden tools were called".into());
            }
        }

        // Required params
        for param in &ec.required_params {
            let param_pass = Self::check_param(param, tool_calls);
            if !param_pass {
                all_pass = false;
                details.push(format!(
                    "Parameter '{}' not satisfied in any tool call",
                    param.key
                ));
            }
        }
        if !ec.required_params.is_empty() && !details.iter().any(|d| d.contains("Parameter")) {
            details.push("All parameter requirements were satisfied".into());
        }

        // Evidence consistency (stub)
        if ec.evidence_consistency {
            details.push("Evidence consistency check not yet implemented".into());
        }

        ExecutionCheckResult {
            scenario: ec.scenario.clone(),
            passed: all_pass,
            detail: details.join("; "),
        }
    }

    /// Check a single `ParamMatcher` against tool call arguments.
    fn check_param(param: &ParamMatcher, tool_calls: &[ToolCallSummary]) -> bool {
        for tc in tool_calls {
            // Try to parse args as JSON and check the key
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&tc.args) {
                if let Some(field) = val.get(&param.key) {
                    if let Some(expected) = &param.equals {
                        if field.as_str() == Some(expected) || field == &serde_json::json!(expected) {
                            return true;
                        }
                    }
                    if let Some(substr) = &param.contains {
                        if let Some(s) = field.as_str() {
                            if s.contains(substr) {
                                return true;
                            }
                        }
                        // Also check the raw JSON representation
                        if field.to_string().contains(substr) {
                            return true;
                        }
                    }
                    // If neither equals nor contains is set, key presence is enough
                    if param.equals.is_none() && param.contains.is_none() {
                        return true;
                    }
                }
            }
        }
        false
    }

    // ── Quality ──────────────────────────────────────────────────────

    fn evaluate_quality(qc: &QualityCase, response: &str) -> QualityCheckResult {
        let mut details = Vec::new();
        let mut all_pass = true;

        // must_contain
        for required in &qc.must_contain {
            if response.contains(required.as_str()) {
                details.push(format!("Contains '{}' ✓", required));
            } else {
                all_pass = false;
                details.push(format!("Missing required content '{}'", required));
            }
        }

        // must_not_contain
        for forbidden in &qc.must_not_contain {
            if response.contains(forbidden.as_str()) {
                all_pass = false;
                details.push(format!("Contains forbidden content '{}'", forbidden));
            } else {
                details.push(format!("Correctly avoids '{}' ✓", forbidden));
            }
        }

        // min_length
        if let Some(min) = qc.min_length {
            if response.len() >= min {
                details.push(format!("Response length {} >= {} ✓", response.len(), min));
            } else {
                all_pass = false;
                details.push(format!(
                    "Response too short: {} chars (minimum: {})",
                    response.len(),
                    min
                ));
            }
        }

        QualityCheckResult {
            name: qc.name.clone(),
            passed: all_pass,
            detail: details.join("; "),
        }
    }

    // ── Resilience (§04) ───────────────────────────────────────────────

    /// Evaluate a single resilience case via retroactive failure detection.
    ///
    /// Checks whether the expected failure mode occurred in the tool call
    /// results, and whether the agent handled it with the expected degradation.
    fn evaluate_resilience(
        rc: &ResilienceCase,
        tool_calls: &[ToolCallSummary],
        response: &str,
    ) -> ResilienceCheckResult {
        let failure_occurred = Self::detect_failure(&rc.inject, tool_calls);

        if !failure_occurred {
            return ResilienceCheckResult {
                passed: true,
                detail: format!(
                    "Failure mode '{:?}' not observed — tool calls all succeeded. \
                     Consider whether the test input triggers the expected failure.",
                    rc.inject
                ),
            };
        }

        let handled = Self::check_degradation(&rc.expect, tool_calls, response);

        if handled {
            ResilienceCheckResult {
                passed: true,
                detail: format!(
                    "Failure '{:?}' occurred and was handled with expected degradation '{:?}'",
                    rc.inject, rc.expect
                ),
            }
        } else {
            ResilienceCheckResult {
                passed: false,
                detail: format!(
                    "Failure '{:?}' occurred but expected degradation '{:?}' was not observed",
                    rc.inject, rc.expect
                ),
            }
        }
    }

    /// Detect whether a specific `FailureMode` occurred in tool call results.
    fn detect_failure(mode: &FailureMode, tool_calls: &[ToolCallSummary]) -> bool {
        match mode {
            FailureMode::Timeout => {
                tool_calls.iter().any(|tc| {
                    !tc.success && tc.result.is_empty() || tc.duration_ms > 30_000
                })
            }
            FailureMode::Error(pattern) => {
                tool_calls.iter().any(|tc| !tc.success && tc.result.contains(pattern.as_str()))
            }
            FailureMode::EmptyResult => {
                tool_calls.iter().any(|tc| tc.result.trim().is_empty())
            }
        }
    }

    /// Check whether the agent's degradation behavior meets expectations.
    fn check_degradation(
        expect: &DegradeExpectation,
        tool_calls: &[ToolCallSummary],
        response: &str,
    ) -> bool {
        match expect {
            DegradeExpectation::GracefulMessage(expected_msg) => {
                response.contains(expected_msg.as_str())
            }
            DegradeExpectation::Retry => {
                let mut name_counts: HashMap<&str, usize> = HashMap::new();
                for tc in tool_calls {
                    *name_counts.entry(tc.name.as_str()).or_insert(0) += 1;
                }
                name_counts.values().any(|&count| count > 1)
            }
            DegradeExpectation::Fallback(fallback_tool) => {
                tool_calls.iter().any(|tc| tc.name == *fallback_tool)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(name: &str, args: &str) -> ToolCallSummary {
        ToolCallSummary {
            name: name.into(),
            args: args.into(),
            result: String::new(),
            success: true,
            duration_ms: 0,
        }
    }

    // ── Trigger ──────────────────────────────────────────────────────

    #[test]
    fn test_trigger_should_pass() {
        let calls = vec![make_tool("web_search", r#"{"query": "test"}"#)];
        let tc = TriggerCase {
            should_trigger: Some(crate::eval::dataset::ShouldTriggerCase {
                input: "search".into(),
                expect_tool: "web_search".into(),
            }),
            should_not_trigger: None,
        };
        let r = SkillScorer::evaluate_trigger(&tc, &calls);
        assert!(r.passed, "{}", r.detail);
        assert_eq!(r.case_label, "should_trigger(web_search)");
    }

    #[test]
    fn test_trigger_should_fail() {
        let calls = vec![make_tool("shell", "echo hi")];
        let tc = TriggerCase {
            should_trigger: Some(crate::eval::dataset::ShouldTriggerCase {
                input: "search".into(),
                expect_tool: "web_search".into(),
            }),
            should_not_trigger: None,
        };
        let r = SkillScorer::evaluate_trigger(&tc, &calls);
        assert!(!r.passed, "{}", r.detail);
    }

    #[test]
    fn test_trigger_should_not_pass() {
        let calls = vec![make_tool("web_search", "query")];
        let tc = TriggerCase {
            should_trigger: None,
            should_not_trigger: Some(crate::eval::dataset::NoTriggerCase {
                input: "hello".into(),
                expect_no_tool: "shell".into(),
            }),
        };
        let r = SkillScorer::evaluate_trigger(&tc, &calls);
        assert!(r.passed, "{}", r.detail);
    }

    #[test]
    fn test_trigger_should_not_fail() {
        let calls = vec![make_tool("shell", "rm -rf /")];
        let tc = TriggerCase {
            should_trigger: None,
            should_not_trigger: Some(crate::eval::dataset::NoTriggerCase {
                input: "hello".into(),
                expect_no_tool: "shell".into(),
            }),
        };
        let r = SkillScorer::evaluate_trigger(&tc, &calls);
        assert!(!r.passed, "{}", r.detail);
    }

    #[test]
    fn test_trigger_no_tool_at_all() {
        let calls: Vec<ToolCallSummary> = vec![];
        let tc = TriggerCase {
            should_trigger: None,
            should_not_trigger: Some(crate::eval::dataset::NoTriggerCase {
                input: "hello".into(),
                expect_no_tool: String::new(), // empty means no tools at all
            }),
        };
        let r = SkillScorer::evaluate_trigger(&tc, &calls);
        assert!(r.passed, "{}", r.detail);
    }

    // ── Execution ────────────────────────────────────────────────────

    #[test]
    fn test_execution_required_tools_pass() {
        let calls = vec![make_tool("web_search", "query"), make_tool("web_fetch", "url")];
        let ec = ExecutionCase {
            scenario: "test".into(),
            required_tools: vec!["web_search".into()],
            forbidden_tools: vec![],
            required_params: vec![],
            evidence_consistency: false,
        };
        let r = SkillScorer::evaluate_execution(&ec, &calls);
        assert!(r.passed, "{}", r.detail);
    }

    #[test]
    fn test_execution_required_tools_fail() {
        let calls = vec![make_tool("shell", "echo hi")];
        let ec = ExecutionCase {
            scenario: "test".into(),
            required_tools: vec!["web_search".into()],
            forbidden_tools: vec![],
            required_params: vec![],
            evidence_consistency: false,
        };
        let r = SkillScorer::evaluate_execution(&ec, &calls);
        assert!(!r.passed, "{}", r.detail);
    }

    #[test]
    fn test_execution_forbidden_tools_fail() {
        let calls = vec![make_tool("shell", "rm -rf")];
        let ec = ExecutionCase {
            scenario: "test".into(),
            required_tools: vec![],
            forbidden_tools: vec!["shell".into()],
            required_params: vec![],
            evidence_consistency: false,
        };
        let r = SkillScorer::evaluate_execution(&ec, &calls);
        assert!(!r.passed, "{}", r.detail);
    }

    #[test]
    fn test_execution_param_match() {
        let calls = vec![make_tool(
            "web_search",
            r#"{"query": "latest AI news"}"#,
        )];
        let ec = ExecutionCase {
            scenario: "param check".into(),
            required_tools: vec![],
            forbidden_tools: vec![],
            required_params: vec![ParamMatcher {
                key: "query".into(),
                contains: Some("AI".into()),
                equals: None,
            }],
            evidence_consistency: false,
        };
        let r = SkillScorer::evaluate_execution(&ec, &calls);
        assert!(r.passed, "{}", r.detail);
    }

    #[test]
    fn test_execution_param_fail() {
        let calls = vec![make_tool(
            "web_search",
            r#"{"query": "weather"}"#,
        )];
        let ec = ExecutionCase {
            scenario: "param check".into(),
            required_tools: vec![],
            forbidden_tools: vec![],
            required_params: vec![ParamMatcher {
                key: "query".into(),
                contains: Some("AI".into()),
                equals: None,
            }],
            evidence_consistency: false,
        };
        let r = SkillScorer::evaluate_execution(&ec, &calls);
        assert!(!r.passed, "{}", r.detail);
    }

    // ── Quality ──────────────────────────────────────────────────────

    #[test]
    fn test_quality_must_contain_pass() {
        let qc = QualityCase {
            name: "test".into(),
            must_contain: vec!["hello".into(), "world".into()],
            must_not_contain: vec![],
            min_length: None,
        };
        let r = SkillScorer::evaluate_quality(&qc, "hello world");
        assert!(r.passed, "{}", r.detail);
    }

    #[test]
    fn test_quality_must_contain_fail() {
        let qc = QualityCase {
            name: "test".into(),
            must_contain: vec!["hello".into(), "world".into(), "missing".into()],
            must_not_contain: vec![],
            min_length: None,
        };
        let r = SkillScorer::evaluate_quality(&qc, "hello world");
        assert!(!r.passed, "{}", r.detail);
    }

    #[test]
    fn test_quality_must_not_contain_fail() {
        let qc = QualityCase {
            name: "test".into(),
            must_contain: vec![],
            must_not_contain: vec!["badword".into()],
            min_length: None,
        };
        let r = SkillScorer::evaluate_quality(&qc, "this contains badword");
        assert!(!r.passed, "{}", r.detail);
    }

    #[test]
    fn test_quality_min_length_pass() {
        let qc = QualityCase {
            name: "test".into(),
            must_contain: vec![],
            must_not_contain: vec![],
            min_length: Some(5),
        };
        let r = SkillScorer::evaluate_quality(&qc, "hello world");
        assert!(r.passed, "{}", r.detail);
    }

    #[test]
    fn test_quality_min_length_fail() {
        let qc = QualityCase {
            name: "test".into(),
            must_contain: vec![],
            must_not_contain: vec![],
            min_length: Some(100),
        };
        let r = SkillScorer::evaluate_quality(&qc, "short");
        assert!(!r.passed, "{}", r.detail);
    }

    // ── Full evaluation ──────────────────────────────────────────────

    #[test]
    fn test_evaluate_empty_design() {
        let design = SkillEvalDesign {
            trigger: vec![],
            execution: vec![],
            quality: vec![],
            resilience: vec![],
        };
        let calls = vec![make_tool("shell", "echo hi")];
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(SkillScorer::evaluate(&design, &calls, "ok"));
        assert!(result.passed);
        assert!(result.trigger_results.is_empty());
        assert!(result.execution_results.is_empty());
        assert!(result.quality_results.is_empty());
        assert!(result.resilience_results.is_empty());
    }

    #[test]
    fn test_evaluate_full_design() {
        let design = SkillEvalDesign {
            trigger: vec![
                TriggerCase {
                    should_trigger: Some(crate::eval::dataset::ShouldTriggerCase {
                        input: "search test".into(),
                        expect_tool: "web_search".into(),
                    }),
                    should_not_trigger: None,
                },
            ],
            execution: vec![
                ExecutionCase {
                    scenario: "core path".into(),
                    required_tools: vec!["web_search".into()],
                    forbidden_tools: vec!["shell".into()],
                    required_params: vec![ParamMatcher {
                        key: "query".into(),
                        contains: Some("test".into()),
                        equals: None,
                    }],
                    evidence_consistency: false,
                },
            ],
            quality: vec![
                QualityCase {
                    name: "response quality".into(),
                    must_contain: vec!["result".into()],
                    must_not_contain: vec![],
                    min_length: Some(10),
                },
            ],
            resilience: vec![],
        };
        let calls = vec![make_tool(
            "web_search",
            r#"{"query": "test search"}"#,
        )];
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(SkillScorer::evaluate(&design, &calls, "here is the result"));
        assert!(result.passed, "trigger={:?}, exec={:?}, quality={:?}",
            result.trigger_results, result.execution_results, result.quality_results);
    }

    // ── Resilience ────────────────────────────────────────────────────

    fn make_failed_tool(name: &str, result: &str, duration_ms: u64) -> ToolCallSummary {
        ToolCallSummary {
            name: name.into(),
            args: String::new(),
            result: result.into(),
            success: false,
            duration_ms,
        }
    }

    #[test]
    fn test_detect_timeout_by_duration() {
        let calls = vec![make_failed_tool("web_search", "timeout", 35_000)];
        assert!(SkillScorer::detect_failure(&FailureMode::Timeout, &calls));
    }

    #[test]
    fn test_detect_timeout_by_empty_result() {
        let calls = vec![make_failed_tool("web_search", "", 0)];
        assert!(SkillScorer::detect_failure(&FailureMode::Timeout, &calls));
    }

    #[test]
    fn test_detect_error() {
        let calls = vec![make_failed_tool("web_search", "API rate limit exceeded", 100)];
        assert!(SkillScorer::detect_failure(
            &FailureMode::Error("rate limit".into()),
            &calls,
        ));
    }

    #[test]
    fn test_detect_error_no_match() {
        let calls = vec![make_failed_tool("web_search", "connection refused", 100)];
        assert!(!SkillScorer::detect_failure(
            &FailureMode::Error("timeout".into()),
            &calls,
        ));
    }

    #[test]
    fn test_detect_empty_result() {
        let mut tc = make_failed_tool("web_search", "", 100);
        tc.success = true; // succeeded but returned nothing
        let calls = vec![tc];
        assert!(SkillScorer::detect_failure(&FailureMode::EmptyResult, &calls));
    }

    #[test]
    fn test_degradation_graceful_message() {
        let calls = vec![];
        assert!(SkillScorer::check_degradation(
            &DegradeExpectation::GracefulMessage("抱歉，没有找到".into()),
            &calls,
            "抱歉，没有找到相关结果",
        ));
    }

    #[test]
    fn test_degradation_graceful_message_fail() {
        let calls = vec![];
        assert!(!SkillScorer::check_degradation(
            &DegradeExpectation::GracefulMessage("try again".into()),
            &calls,
            "some unrelated response",
        ));
    }

    #[test]
    fn test_degradation_retry() {
        let calls = vec![
            make_tool("web_search", "query"),
            make_tool("web_fetch", "url"),
            make_tool("web_search", "query"), // retry
        ];
        assert!(SkillScorer::check_degradation(
            &DegradeExpectation::Retry,
            &calls,
            "",
        ));
    }

    #[test]
    fn test_degradation_no_retry() {
        let calls = vec![
            make_tool("web_search", "query"),
            make_tool("web_fetch", "url"),
        ];
        assert!(!SkillScorer::check_degradation(
            &DegradeExpectation::Retry,
            &calls,
            "",
        ));
    }

    #[test]
    fn test_degradation_fallback() {
        let calls = vec![make_tool("web_fetch", "url")];
        assert!(SkillScorer::check_degradation(
            &DegradeExpectation::Fallback("web_fetch".into()),
            &calls,
            "",
        ));
    }

    #[test]
    fn test_resilience_no_failure_detected() {
        let rc = crate::eval::dataset::ResilienceCase {
            inject: FailureMode::Timeout,
            expect: DegradeExpectation::GracefulMessage("sorry".into()),
        };
        let calls = vec![make_tool("web_search", "query")]; // all healthy
        let r = SkillScorer::evaluate_resilience(&rc, &calls, "hello");
        assert!(r.passed, "{}", r.detail);
        assert!(r.detail.contains("not observed"));
    }

    #[test]
    fn test_resilience_failure_handled() {
        let rc = crate::eval::dataset::ResilienceCase {
            inject: FailureMode::Timeout,
            expect: DegradeExpectation::GracefulMessage("sorry".into()),
        };
        let calls = vec![make_failed_tool("web_search", "", 35_000)];
        let r = SkillScorer::evaluate_resilience(&rc, &calls, "sorry, something went wrong");
        assert!(r.passed, "{}", r.detail);
    }

    #[test]
    fn test_resilience_failure_unhandled() {
        let rc = crate::eval::dataset::ResilienceCase {
            inject: FailureMode::Error("timeout".into()),
            expect: DegradeExpectation::GracefulMessage("please retry".into()),
        };
        let calls = vec![make_failed_tool("web_search", "timeout error", 1_000)];
        let r = SkillScorer::evaluate_resilience(&rc, &calls, "unrelated response");
        assert!(!r.passed, "{}", r.detail);
    }
}