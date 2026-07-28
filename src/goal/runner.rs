//! Goal runner — the sub-agent that executes a [`GoalPlan`] in a loop.
//!
//! The [`GoalRunner`] spawns as a background task, running iterations of
//! "agent acts → check conditions → feedback → repeat" until all conditions
//! pass or a guardrail trips.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::goal::condition::CheckResult;
use crate::goal::event::GoalEvent;
use crate::goal::persist;
use crate::goal::plan::GoalPlan;
use crate::model_router::ModelRouter;
use crate::providers::{Message, ToolDefinition};
use crate::tools::ToolContext;
use crate::tools::ToolRegistry;
use crate::Result;

/// Maximum consecutive identical failures before loop detection triggers.
const MAX_CONSECUTIVE_IDENTICAL_FAILURES: usize = 3;

/// Maximum number of LLM → tool → LLM iterations within a single agent round.
const MAX_TOOL_ITERATIONS: usize = 25;

/// A background goal runner that acts and checks conditions in a loop.
pub struct GoalRunner {
    /// Unique identifier for this goal run.
    pub id: String,
    /// Session ID of the parent (originating) session.
    pub parent_session_id: String,
    /// The goal plan with conditions.
    pub plan: GoalPlan,
    /// Current round number (1-indexed).
    pub round: usize,
    /// Tool registry for tool execution.
    tools: Arc<ToolRegistry>,
    /// Model router for LLM completions.
    model_router: Arc<ModelRouter>,
    /// Optional model override for the sub-agent (from GoalPlan).
    model_override: Option<String>,
    /// History of round results (for loop detection).
    pub condition_history: Vec<RoundResult>,
    /// Cancel signal.
    cancel: CancellationToken,
    /// Channel to emit events back to the originating session.
    event_tx: tokio::sync::mpsc::UnboundedSender<GoalEvent>,
    /// Optional persisted state store for checkpoint persistence.
    store: Option<crate::goal::persist::SharedGoalStore>,
}

#[derive(Debug, Clone)]
pub struct RoundResult {
    pub round: usize,
    pub results: Vec<CheckResult>,
}

impl GoalRunner {
    /// Create a new goal runner.
    pub fn new(
        id: impl Into<String>,
        parent_session_id: impl Into<String>,
        plan: GoalPlan,
        tools: Arc<ToolRegistry>,
        model_router: Arc<ModelRouter>,
        event_tx: tokio::sync::mpsc::UnboundedSender<GoalEvent>,
    ) -> Self {
        let model_override = plan.model_override.clone();
        Self {
            id: id.into(),
            parent_session_id: parent_session_id.into(),
            plan,
            round: 0,
            tools,
            model_router,
            model_override,
            condition_history: Vec::new(),
            cancel: CancellationToken::new(),
            event_tx,
            store: None,
        }
    }

    /// Attach a persistence store so the runner checkpoints state after each
    /// round.
    pub fn with_store(mut self, store: crate::goal::persist::SharedGoalStore) -> Self {
        self.store = Some(store);
        self
    }

    /// Set the initial round and condition history (used when restoring from
    /// persistence).
    pub fn with_progress(mut self, round: usize, condition_history: Vec<RoundResult>) -> Self {
        self.round = round;
        self.condition_history = condition_history;
        self
    }

    /// Get a cancel token for this runner (for external cancellation).
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Cancel the goal runner.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Run the goal execution loop.
    ///
    /// 1. Emits `Started` event (or `Started` with resume note if restoring).
    /// 2. Loop: emit `Retry` → agent acts → check conditions → emit `Check`
    /// 3. If all pass: emit `Done` and return.
    /// 4. If guardrail: emit `Aborted` and return.
    pub async fn run(mut self) {
        let is_resume = self.round > 0;

        if !is_resume {
            // Emit started event (fresh goal).
            let conditions_desc: Vec<String> =
                self.plan.conditions.iter().map(|c| c.to_string()).collect();
            self.emit(GoalEvent::Started {
                id: self.id.clone(),
                description: self.plan.description.clone(),
                conditions: conditions_desc,
                max_rounds: self.plan.max_rounds,
            });
            self.round = 1;
        } else {
            // Resuming a persisted goal — emit a resume notification.
            self.emit(GoalEvent::Started {
                id: self.id.clone(),
                description: format!(
                    "{} (resumed, round {}/{})",
                    self.plan.description, self.round, self.plan.max_rounds
                ),
                conditions: self.plan.conditions.iter().map(|c| c.to_string()).collect(),
                max_rounds: self.plan.max_rounds,
            });
        }

        while self.round <= self.plan.max_rounds {
            // Check cancellation.
            if self.cancel.is_cancelled() {
                self.emit(GoalEvent::Aborted {
                    reason: "cancelled".to_string(),
                    round: self.round,
                    results: Vec::new(),
                });
                self.cleanup_store().await;
                return;
            }

            // Build feedback for the agent from previous round.
            let feedback = self.build_feedback();

            // Emit retry event (or initial started feedback).
            if self.round > 1 || !feedback.is_empty() {
                self.emit(GoalEvent::Retry { round: self.round, feedback });
            }

            // Agent acts: run the agent with the goal context.
            if let Err(e) = self.run_agent_round().await {
                tracing::warn!("[goal {}] Agent round failed: {}", self.id, e);
                self.emit(GoalEvent::Aborted {
                    reason: format!("agent_error: {}", e),
                    round: self.round,
                    results: Vec::new(),
                });
                self.cleanup_store().await;
                return;
            }

            // Check all conditions.
            let mut results: Vec<CheckResult> = Vec::new();
            for condition in &self.plan.conditions {
                let result = condition.check().await;
                results.push(result);
            }

            let passed = results.iter().filter(|r| r.passed).count();
            let total = results.len();

            self.emit(GoalEvent::Check {
                round: self.round,
                results: results.clone(),
                passed,
                total,
            });

            // Check if all passed.
            if passed == total {
                let summary = format!(
                    "目标完成: {} ({} 轮, {}/{} 条件通过)",
                    self.plan.description, self.round, passed, total
                );
                self.emit(GoalEvent::Done {
                    total_rounds: self.round,
                    all_passed: true,
                    summary,
                });
                // Clean up persisted state on successful completion.
                if let Some(ref store) = self.store {
                    let goal_id = self.id.clone();
                    let store_clone = Arc::clone(store);
                    tokio::spawn(async move {
                        let store = store_clone.read().await;
                        store.delete(&goal_id).await;
                    });
                }
                return;
            }

            // Save checkpoint after each round.
            self.save_checkpoint().await;

            // Loop detection: consecutive identical failures.
            self.condition_history.push(RoundResult {
                round: self.round,
                results: results.clone(),
            });
            if self.detect_loop() {
                self.emit(GoalEvent::Aborted {
                    reason: "loop_detected: same conditions failed 3 rounds in a row with \
                             identical output"
                        .to_string(),
                    round: self.round,
                    results,
                });
                self.cleanup_store().await;
                return;
            }

            self.round += 1;
        }

        // Max rounds reached.
        let results: Vec<CheckResult> = self
            .condition_history
            .last()
            .map(|r| r.results.clone())
            .unwrap_or_default();
        self.emit(GoalEvent::Aborted {
            reason: format!("max_rounds: reached {} rounds", self.plan.max_rounds),
            round: self.plan.max_rounds,
            results,
        });
        self.cleanup_store().await;
    }

    /// Run the agent for one round.
    ///
    /// Builds a goal-specific system prompt + current progress, sends it to the
    /// LLM via the model router, and executes any tool calls the LLM makes.
    /// Repeats (LLM → tools → LLM → …) up to [`MAX_TOOL_ITERATIONS`]
    /// iterations.
    async fn run_agent_round(&self) -> Result<()> {
        // Build system prompt describing the goal and available tools.
        let system_prompt = self.build_agent_system_prompt();

        // Build the user message with current progress / feedback.
        let user_message = self.build_feedback();

        // Get tool definitions from the registry.
        let tool_defs: Vec<ToolDefinition> = self
            .tools
            .get_definitions()
            .into_iter()
            .map(|f| ToolDefinition {
                tool_type: "function".to_string(),
                function: f,
            })
            .collect();

        let mut messages = vec![
            Message::system(&system_prompt),
            Message::user(&user_message),
        ];

        let model = self.model_override.as_deref().unwrap_or("default");

        // Create a tool context for tool execution.
        let tool_ctx = ToolContext::new("goal_runner", &self.id)
            .with_workspace_root(crate::dirs::workspace_data_dir())
            .with_model_name(model.to_string())
            .with_provider_name("model_router");

        for iteration in 0..MAX_TOOL_ITERATIONS {
            // Check cancellation between iterations.
            if self.cancel.is_cancelled() {
                tracing::info!(
                    "[goal {}] Cancelled during agent round (iteration {})",
                    self.id,
                    iteration
                );
                return Ok(());
            }

            let response = self
                .model_router
                .complete(model, messages.clone(), Some(tool_defs.clone()))
                .await
                .map_err(|e| {
                    crate::error::SyscityError::Internal(format!(
                        "LLM completion failed in goal round {}: {}",
                        self.round, e
                    ))
                })?;

            let has_tool_calls = response
                .message
                .tool_calls
                .as_ref()
                .is_some_and(|c: &Vec<crate::providers::ToolCall>| !c.is_empty());

            if !has_tool_calls {
                // No more tool calls — agent is done responding.
                tracing::debug!(
                    "[goal {}] Agent finished after {} iteration(s)",
                    self.id,
                    iteration + 1
                );
                break;
            }

            // Take the tool calls out of the response (clone to keep response.message
            // valid).
            let tool_calls = response.message.tool_calls.clone().unwrap_or_default();

            // Push the assistant's response (with tool_calls) to the context.
            messages.push(response.message);

            // Execute each tool call sequentially.
            for tc in &tool_calls {
                if self.cancel.is_cancelled() {
                    return Ok(());
                }

                tracing::debug!(
                    "[goal {}] Executing tool: {} (id={})",
                    self.id,
                    tc.function.name,
                    tc.id
                );

                let result = self.tools.execute_call(&tc.function, &tool_ctx).await;

                let result_str = match result {
                    Ok(exec_result) => {
                        if exec_result.success {
                            exec_result.output
                        } else {
                            format!("Error: {}", exec_result.error.unwrap_or_default())
                        }
                    }
                    Err(e) => format!("Tool execution error: {}", e),
                };

                messages.push(Message::tool(result_str, &tc.id));
            }
        }

        tracing::info!("[goal {}] Agent round {} completed", self.id, self.round);
        Ok(())
    }

    /// Build the system prompt for the sub-agent with goal context.
    fn build_agent_system_prompt(&self) -> String {
        let conditions: Vec<String> = self
            .plan
            .conditions
            .iter()
            .map(|c| format!("  [ ] {}", c))
            .collect();

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
4. You may call tools multiple times to iterate and refine your work.
5. Working directory: {}
6. Do not modify .git directories or sensitive configuration files.
7. When done, reply with a brief completion message."#,
            self.plan.description,
            conditions.join("\n"),
            crate::dirs::workspace_data_dir().display(),
        )
    }

    /// Save a checkpoint of the current goal state to the persistence store.
    async fn save_checkpoint(&self) {
        if let Some(ref store) = self.store {
            let store_guard = store.read().await;
            let state = persist::to_persisted(
                &self.id,
                &self.parent_session_id,
                &self.plan,
                self.round,
                &self.condition_history,
            );
            if let Err(e) = store_guard.save(&state).await {
                tracing::warn!("[goal {}] Failed to save checkpoint: {}", self.id, e);
            }
        }
    }

    /// Delete the persisted state file for this goal on terminal state.
    async fn cleanup_store(&self) {
        if let Some(ref store) = self.store {
            let goal_id = self.id.clone();
            let store_owned = Arc::clone(store);
            tokio::spawn(async move {
                let store_guard = store_owned.read().await;
                store_guard.delete(&goal_id).await;
            });
        }
    }

    /// Build feedback string from previous round results.
    fn build_feedback(&self) -> String {
        if self.condition_history.is_empty() {
            let conditions: Vec<String> = self
                .plan
                .conditions
                .iter()
                .map(|c| format!("  [ ] {}", c))
                .collect();
            return format!(
                "目标: {}\n\n需要满足以下条件:\n{}",
                self.plan.description,
                conditions.join("\n")
            );
        }

        let last = match self.condition_history.last() {
            Some(last) => last,
            None => return "尚无执行记录".to_string(),
        };
        let passed = last.results.iter().filter(|r| r.passed).count();
        let total = last.results.len();

        let mut lines = vec![format!(
            "第 {} 轮结果: {}/{} 条件通过",
            last.round, passed, total
        )];

        for r in &last.results {
            let icon = if r.passed { "✓" } else { "✗" };
            lines.push(format!("  {} {} — {}", icon, r.condition, r.detail));
        }

        if passed < total {
            lines.push(String::new());
            lines.push("请修复失败的检查项。".to_string());
        }

        lines.join("\n")
    }

    /// Detect whether the runner is in a loop: same conditions failing with
    /// identical failure signatures for MAX_CONSECUTIVE_IDENTICAL_FAILURES
    /// rounds.
    fn detect_loop(&self) -> bool {
        if self.condition_history.len() < MAX_CONSECUTIVE_IDENTICAL_FAILURES {
            return false;
        }

        let recent = &self.condition_history
            [self.condition_history.len() - MAX_CONSECUTIVE_IDENTICAL_FAILURES..];

        // All recent rounds must have the same failing condition signatures.
        let baseline: Vec<String> = recent[0]
            .results
            .iter()
            .filter(|r| !r.passed)
            .map(|r| r.condition.failure_signature(&r.actual))
            .collect();

        if baseline.is_empty() {
            return false; // all passed, not a loop
        }

        recent.iter().all(|round| {
            let sigs: Vec<String> = round
                .results
                .iter()
                .filter(|r| !r.passed)
                .map(|r| r.condition.failure_signature(&r.actual))
                .collect();
            sigs == baseline
        })
    }

    fn emit(&self, event: GoalEvent) {
        if let Err(e) = self.event_tx.send(event) {
            tracing::warn!("[goal {}] Failed to emit event: {}", self.id, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::condition::Comparison;
    use crate::goal::condition::GoalCondition;
    use crate::model_router::ModelRouterConfig;

    fn make_router() -> Arc<ModelRouter> {
        Arc::new(ModelRouter::new(ModelRouterConfig::default()))
    }

    fn make_tools() -> Arc<ToolRegistry> {
        Arc::new(ToolRegistry::new())
    }

    fn make_runner(plan: GoalPlan) -> GoalRunner {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        GoalRunner::new("test_goal", "test_session", plan, make_tools(), make_router(), tx)
    }

    fn make_plan(description: &str) -> GoalPlan {
        GoalPlan::new(description).with_condition(GoalCondition::ExitCode {
            command: "true".to_string(),
            expected: Some(0),
        })
    }

    #[test]
    fn test_with_progress_sets_round_and_history() {
        let plan = make_plan("test");
        let runner = make_runner(plan).with_progress(
            3,
            vec![RoundResult {
                round: 1,
                results: vec![CheckResult {
                    condition: GoalCondition::ExitCode {
                        command: "true".to_string(),
                        expected: Some(0),
                    },
                    passed: true,
                    actual: "exit code: 0".to_string(),
                    detail: "passed".to_string(),
                }],
            }],
        );
        assert_eq!(runner.round, 3);
        assert_eq!(runner.condition_history.len(), 1);
        assert_eq!(runner.condition_history[0].round, 1);
    }

    #[test]
    fn test_build_feedback_empty_history() {
        let plan = make_plan("write tests");
        let runner = make_runner(plan);
        let feedback = runner.build_feedback();
        assert!(feedback.contains("write tests"));
        assert!(feedback.contains("[ ]"));
    }

    #[test]
    fn test_build_feedback_with_results() {
        let plan = make_plan("run tests");
        let mut runner = make_runner(plan);
        runner.condition_history.push(RoundResult {
            round: 1,
            results: vec![
                CheckResult {
                    condition: GoalCondition::ExitCode {
                        command: "true".to_string(),
                        expected: Some(0),
                    },
                    passed: true,
                    actual: "exit code: 0".to_string(),
                    detail: "passed".to_string(),
                },
                CheckResult {
                    condition: GoalCondition::Numeric {
                        command: "echo 2".to_string(),
                        operator: Comparison::Ge,
                        threshold: 5.0,
                    },
                    passed: false,
                    actual: "2".to_string(),
                    detail: "got 2, expected >= 5".to_string(),
                },
            ],
        });
        let feedback = runner.build_feedback();
        assert!(feedback.contains("1/2"));
        assert!(feedback.contains("✓"));
        assert!(feedback.contains("✗"));
        assert!(feedback.contains("请修复"));
    }

    #[test]
    fn test_detect_loop_not_enough_rounds() {
        let plan = make_plan("test");
        let runner = make_runner(plan);
        // 0 entries
        assert!(!runner.detect_loop());
    }

    #[test]
    fn test_detect_loop_two_rounds_not_enough() {
        let plan = make_plan("test");
        let mut runner = make_runner(plan);
        let fail_result = CheckResult {
            condition: GoalCondition::ExitCode {
                command: "false".to_string(),
                expected: Some(0),
            },
            passed: false,
            actual: "exit code: 1".to_string(),
            detail: "failed".to_string(),
        };
        runner.condition_history.push(RoundResult {
            round: 1,
            results: vec![fail_result.clone()],
        });
        runner.condition_history.push(RoundResult {
            round: 2,
            results: vec![fail_result],
        });
        assert!(!runner.detect_loop());
    }

    #[test]
    fn test_detect_loop_three_identical_failures() {
        let plan = make_plan("test");
        let mut runner = make_runner(plan);
        let fail_result = CheckResult {
            condition: GoalCondition::ExitCode {
                command: "false".to_string(),
                expected: Some(0),
            },
            passed: false,
            actual: "exit code: 1".to_string(),
            detail: "failed".to_string(),
        };
        for round in 1..=3 {
            runner.condition_history.push(RoundResult {
                round,
                results: vec![fail_result.clone()],
            });
        }
        assert!(runner.detect_loop());
    }

    #[test]
    fn test_detect_loop_different_failures() {
        let plan = make_plan("test");
        let mut runner = make_runner(plan);
        for round in 1..=3 {
            let actual = format!("exit code: {}", round);
            runner.condition_history.push(RoundResult {
                round,
                results: vec![CheckResult {
                    condition: GoalCondition::ExitCode {
                        command: "false".to_string(),
                        expected: Some(0),
                    },
                    passed: false,
                    actual,
                    detail: "failed".to_string(),
                }],
            });
        }
        assert!(!runner.detect_loop());
    }

    #[test]
    fn test_detect_loop_all_passed_not_loop() {
        let plan = make_plan("test");
        let mut runner = make_runner(plan);
        let pass_result = CheckResult {
            condition: GoalCondition::ExitCode {
                command: "true".to_string(),
                expected: Some(0),
            },
            passed: true,
            actual: "exit code: 0".to_string(),
            detail: "passed".to_string(),
        };
        for round in 1..=3 {
            runner.condition_history.push(RoundResult {
                round,
                results: vec![pass_result.clone()],
            });
        }
        assert!(!runner.detect_loop());
    }
}
