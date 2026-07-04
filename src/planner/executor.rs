//! Task executor — runs tasks from a [`Plan`] in topological order.
//!
//! Independent tasks are executed concurrently.  After each task the
//! executor optionally verifies the result and, on failure, retries or
//! triggers a rollback.

use std::collections::HashSet;
use std::sync::Arc;

use super::{DagScheduler, Plan, PlanResult, TaskStatus};
use crate::computer::{
    ActionResult, ComputerAdapter, RollbackManager, VerificationConfig, VerificationEngine,
};
use crate::planner::{ErrorDiagnosisEngine, ExperienceContext, ToolLearningEngine};
use crate::tools::ToolRegistry;

/// Configuration for the task executor.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Maximum number of concurrent tasks.
    pub max_concurrency: usize,
    /// Default verification config.
    pub verification: VerificationConfig,
    /// Whether to enable rollback on failure.
    pub enable_rollback: bool,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 4,
            verification: VerificationConfig::default(),
            enable_rollback: true,
        }
    }
}

/// Executes tasks from a plan.
#[derive(Clone)]
pub struct TaskExecutor {
    adapter: Arc<dyn ComputerAdapter>,
    tool_registry: Option<Arc<ToolRegistry>>,
    verifier: VerificationEngine,
    config: ExecutorConfig,
    /// Optional execution controller for pause / resume / step / cancel.
    execution_controller: Option<Arc<crate::acp::ExecutionController>>,
    /// Diagnoses failures and suggests remediation steps.
    diagnosis_engine: ErrorDiagnosisEngine,
    /// Records tool execution experience for future alternative suggestions.
    learning_engine: Option<Arc<ToolLearningEngine>>,
}

impl TaskExecutor {
    pub fn new(adapter: Arc<dyn ComputerAdapter>, verifier: VerificationEngine) -> Self {
        Self {
            adapter,
            tool_registry: None,
            verifier,
            config: ExecutorConfig::default(),
            execution_controller: None,
            diagnosis_engine: ErrorDiagnosisEngine::new(),
            learning_engine: None,
        }
    }

    /// Attach a ToolRegistry for executing
    /// [`ToolCall`](crate::computer::DesktopAction::ToolCall) actions.
    pub fn with_tool_registry(mut self, registry: Arc<ToolRegistry>) -> Self {
        self.tool_registry = Some(registry);
        self
    }

    pub fn with_config(mut self, config: ExecutorConfig) -> Self {
        self.config = config;
        self
    }

    /// Attach an execution controller for pause / resume / step / cancel.
    pub fn with_execution_controller(
        mut self,
        controller: Arc<crate::acp::ExecutionController>,
    ) -> Self {
        self.execution_controller = Some(controller);
        self
    }

    /// Attach an error-diagnosis engine for self-correction on failure.
    pub fn with_diagnosis_engine(mut self, engine: ErrorDiagnosisEngine) -> Self {
        self.diagnosis_engine = engine;
        self
    }

    /// Attach a learning engine that records experience and suggests
    /// alternatives.
    pub fn with_learning_engine(mut self, engine: Arc<ToolLearningEngine>) -> Self {
        self.learning_engine = Some(engine);
        self
    }

    /// Execute all tasks in a plan.
    ///
    /// Returns a [`PlanResult`] summarising the outcome.
    pub async fn execute(&self, plan: &mut Plan) -> crate::Result<PlanResult> {
        // Validate the DAG.
        if let Err(e) = DagScheduler::validate(plan) {
            return Ok(PlanResult {
                success: false,
                goal: plan.goal.clone(),
                tasks_completed: 0,
                tasks_failed: 0,
                tasks_rolled_back: 0,
                message: format!("Plan validation failed: {:?}", e),
            });
        }

        let scheduler = DagScheduler::from_plan(plan).map_err(|e| {
            crate::error::SyscityError::Validation(format!("Plan scheduling failed: {:?}", e))
        })?;

        let mut rollback_mgr = if self.config.enable_rollback {
            Some(RollbackManager::new().map_err(|e| {
                crate::error::SyscityError::ExternalService {
                    source: "Failed to create rollback manager".to_string(),
                    cause: Some(Box::new(e)),
                }
            })?)
        } else {
            None
        };

        let mut completed = HashSet::<String>::new();
        let mut failed = HashSet::<String>::new();
        let mut rolled_back = HashSet::<String>::new();

        loop {
            // Check execution controller for pause / resume / step / cancel.
            if let Some(ref ctrl) = self.execution_controller {
                if let Err(reason) = ctrl.check_and_wait().await {
                    return Ok(PlanResult {
                        success: false,
                        goal: plan.goal.clone(),
                        tasks_completed: completed.len(),
                        tasks_failed: failed.len(),
                        tasks_rolled_back: rolled_back.len(),
                        message: reason.to_string(),
                    });
                }
            }

            // Find tasks that are ready to run.
            let ready = scheduler.next_ready(plan, &completed, &failed);

            if ready.is_empty() {
                // Nothing more to run.
                break;
            }

            // Limit concurrency.
            let batch: Vec<String> = ready
                .into_iter()
                .take(self.config.max_concurrency)
                .collect();

            // Execute the batch concurrently.
            // Tasks in a batch are independent (DAG guarantees no inter-dependencies).
            let mut handles: Vec<tokio::task::JoinHandle<(String, TaskExecutionOutcome)>> =
                Vec::with_capacity(batch.len());
            for id in &batch {
                let task = match plan.get_task(id) {
                    Some(t) => t.clone(),
                    None => continue,
                };
                plan.set_status(id, TaskStatus::Running);

                let id_clone = id.clone();
                let adapter = self.adapter.clone();
                let verifier = self.verifier.clone();
                let tool_registry = self.tool_registry.clone();
                let diagnosis_engine = self.diagnosis_engine.clone();
                let learning_engine = self.learning_engine.clone();
                let exec_controller = self.execution_controller.clone();
                let plan_goal = plan.goal.clone();

                handles.push(tokio::spawn(async move {
                    let outcome = execute_task_inner(
                        task,
                        adapter,
                        verifier,
                        tool_registry,
                        diagnosis_engine,
                        learning_engine,
                        exec_controller,
                        plan_goal,
                    )
                    .await;
                    (id_clone, outcome)
                }));
            }

            let mut any_failure = false;
            let mut cancelled = false;
            for handle in handles {
                match handle.await {
                    Ok((id, TaskExecutionOutcome::Success(msg))) => {
                        plan.complete_task(&id, msg);
                        completed.insert(id);
                    }
                    Ok((id, TaskExecutionOutcome::Failure(err))) => {
                        failed.insert(id.clone());
                        any_failure = true;
                        plan.fail_task(&id, err);
                    }
                    Ok((_id, TaskExecutionOutcome::Cancelled(reason))) => {
                        cancelled = true;
                        any_failure = true;
                        tracing::warn!("Task execution cancelled: {}", reason);
                    }
                    Err(e) => {
                        tracing::error!("Task execution panicked: {}", e);
                    }
                }
            }

            if cancelled {
                return Ok(PlanResult {
                    success: false,
                    goal: plan.goal.clone(),
                    tasks_completed: completed.len(),
                    tasks_failed: failed.len(),
                    tasks_rolled_back: rolled_back.len(),
                    message: "Execution cancelled by controller".to_string(),
                });
            }

            // If any task failed and rollback is enabled, roll back completed tasks
            // and abort remaining tasks.
            if any_failure && self.config.enable_rollback {
                let completed_ids: Vec<_> = completed.iter().cloned().collect();
                for id in completed_ids.iter().rev() {
                    rolled_back.insert(id.clone());
                    plan.set_status(id, TaskStatus::RolledBack);
                }
                if let Some(ref mut mgr) = rollback_mgr {
                    if let Err(e) = mgr.rollback().await {
                        tracing::error!("Rollback failed: {}", e);
                    }
                }
                break;
            }
        }

        let tasks_completed = completed.len();
        let tasks_failed = failed.len();
        let tasks_rolled_back = rolled_back.len();
        let success = tasks_failed == 0 && plan.is_complete();

        Ok(PlanResult {
            success,
            goal: plan.goal.clone(),
            tasks_completed,
            tasks_failed,
            tasks_rolled_back,
            message: if success {
                format!("Plan '{}' completed successfully", plan.goal)
            } else {
                format!(
                    "Plan '{}' failed: {} completed, {} failed, {} rolled back",
                    plan.goal, tasks_completed, tasks_failed, tasks_rolled_back
                )
            },
        })
    }
}

/// Return a short snake_case name for a [`DesktopAction`] variant.
fn desktop_action_name(action: &crate::computer::DesktopAction) -> String {
    match action {
        crate::computer::DesktopAction::Screenshot { .. } => "screenshot",
        crate::computer::DesktopAction::Click { .. } => "click",
        crate::computer::DesktopAction::DoubleClick { .. } => "double_click",
        crate::computer::DesktopAction::Type { .. } => "type",
        crate::computer::DesktopAction::KeyPress { .. } => "key_press",
        crate::computer::DesktopAction::Scroll { .. } => "scroll",
        crate::computer::DesktopAction::Drag { .. } => "drag",
        crate::computer::DesktopAction::ReadUiTree { .. } => "read_ui_tree",
        crate::computer::DesktopAction::LaunchApp { .. } => "launch_app",
        crate::computer::DesktopAction::ActivateWindow { .. } => "activate_window",
        crate::computer::DesktopAction::CloseWindow { .. } => "close_window",
        crate::computer::DesktopAction::Wait { .. } => "wait",
        crate::computer::DesktopAction::ClipboardGet => "clipboard_get",
        crate::computer::DesktopAction::ClipboardSet { .. } => "clipboard_set",
        crate::computer::DesktopAction::GetSystemStatus => "get_system_status",
        crate::computer::DesktopAction::ListProcesses { .. } => "list_processes",
        crate::computer::DesktopAction::KillProcess { .. } => "kill_process",
        crate::computer::DesktopAction::WatchDirectory { .. } => "watch_directory",
        crate::computer::DesktopAction::UnwatchDirectory { .. } => "unwatch_directory",
        crate::computer::DesktopAction::WatchFile { .. } => "watch_file",
        crate::computer::DesktopAction::UnwatchFile { .. } => "unwatch_file",
        crate::computer::DesktopAction::ListPorts { .. } => "list_ports",
        crate::computer::DesktopAction::TestPing { .. } => "test_ping",
        crate::computer::DesktopAction::TestTcpConnect { .. } => "test_tcp_connect",
        crate::computer::DesktopAction::ListFirewallRules => "list_firewall_rules",
        crate::computer::DesktopAction::RestartProcess { .. } => "restart_process",
        crate::computer::DesktopAction::SetProcessPriority { .. } => "set_process_priority",
        crate::computer::DesktopAction::KeySequence { .. } => "key_sequence",
        crate::computer::DesktopAction::InstallPackage { .. } => "install_package",
        crate::computer::DesktopAction::BrowseFiles { .. } => "browse_files",
        crate::computer::DesktopAction::EditFile { .. } => "edit_file",
        crate::computer::DesktopAction::ReadFileChunked { .. } => "read_file_chunked",
        crate::computer::DesktopAction::Compress { .. } => "compress",
        crate::computer::DesktopAction::Decompress { .. } => "decompress",
        crate::computer::DesktopAction::TransferFile { .. } => "transfer_file",
        crate::computer::DesktopAction::ToolCall { .. } => "tool_call",
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// Concurrent task execution
// ---------------------------------------------------------------------------

/// Outcome of executing a single task concurrently.
enum TaskExecutionOutcome {
    /// Task completed successfully with an optional result message.
    Success(String),
    /// Task failed after all retries.
    Failure(String),
    /// Execution was cancelled by the controller.
    Cancelled(String),
}

/// Execute a single task with retries and optional verification.
///
/// This is a standalone function (not a method on [`TaskExecutor`]) so that
/// independent tasks can be spawned concurrently via [`tokio::spawn`].
#[allow(clippy::too_many_arguments)]
async fn execute_task_inner(
    task: crate::planner::Task,
    adapter: Arc<dyn ComputerAdapter>,
    verifier: VerificationEngine,
    tool_registry: Option<Arc<ToolRegistry>>,
    diagnosis_engine: ErrorDiagnosisEngine,
    learning_engine: Option<Arc<ToolLearningEngine>>,
    exec_controller: Option<Arc<crate::acp::ExecutionController>>,
    plan_goal: String,
) -> TaskExecutionOutcome {
    let action_name = desktop_action_name(&task.action);

    for attempt in 0..=task.max_retries {
        // Check execution controller before each attempt.
        if let Some(ref ctrl) = exec_controller {
            if let Err(reason) = ctrl.check_and_wait().await {
                return TaskExecutionOutcome::Cancelled(reason.to_string());
            }
        }

        match resolve_action_standalone(&task.action, &adapter, &tool_registry).await {
            Ok(result) => {
                // Verify if criteria are set.
                let verified = if let Some(ref criteria) = task.verification {
                    match verifier.verify(criteria, &result, None).await {
                        Ok(true) => true,
                        Ok(false) => {
                            if attempt < task.max_retries {
                                tracing::warn!(
                                    "Task '{}' verification failed (attempt {}/{}), retrying...",
                                    task.id,
                                    attempt + 1,
                                    task.max_retries + 1
                                );
                                tokio::time::sleep(task.retry_delay).await;
                                continue;
                            }
                            false
                        }
                        Err(e) => {
                            if attempt < task.max_retries {
                                tracing::warn!(
                                    "Task '{}' verification error (attempt {}/{}): {}, retrying...",
                                    task.id,
                                    attempt + 1,
                                    task.max_retries + 1,
                                    e
                                );
                                tokio::time::sleep(task.retry_delay).await;
                                continue;
                            }
                            return TaskExecutionOutcome::Failure(format!(
                                "Verification failed: {}",
                                e
                            ));
                        }
                    }
                } else {
                    true
                };

                if verified {
                    // ── Record success experience ────────────────────────────
                    if let Some(ref learning) = learning_engine {
                        let ctx = ExperienceContext::current(&plan_goal);
                        let _ = learning
                            .record_experience(&action_name, &task.action, true, None, None, &ctx)
                            .await;
                    }

                    return TaskExecutionOutcome::Success(result.message);
                } else {
                    return TaskExecutionOutcome::Failure(
                        "Verification failed after all retries".to_string(),
                    );
                }
            }
            Err(e) => {
                if attempt < task.max_retries {
                    tracing::warn!(
                        "Task '{}' execution failed (attempt {}/{}): {}, retrying...",
                        task.id,
                        attempt + 1,
                        task.max_retries + 1,
                        e
                    );
                    tokio::time::sleep(task.retry_delay).await;
                } else {
                    let error_str = e.to_string();

                    // ── Self-correction: diagnose the failure ─────────────────
                    if let Ok(d) = diagnosis_engine
                        .diagnose(&error_str, &task.description)
                        .await
                    {
                        tracing::info!(
                            "Diagnosis for task '{}': {} (severity: {:?}, confidence: {:.2})",
                            task.id,
                            d.root_causes
                                .first()
                                .map(|r| r.description.as_str())
                                .unwrap_or("unknown"),
                            d.severity,
                            d.confidence
                        );
                    }

                    // ── Check past experience for known alternatives ──────────
                    let mut alternative_suggestion = None;
                    if let Some(ref learning) = learning_engine {
                        let ctx = ExperienceContext::current(&plan_goal);
                        if let Ok(Some(suggestion)) = learning
                            .suggest_alternative(&action_name, &error_str, &ctx)
                            .await
                        {
                            tracing::info!(
                                "ToolLearningEngine suggests alternative for task '{}': {}",
                                task.id,
                                suggestion.alternative
                            );
                            alternative_suggestion = Some(suggestion.alternative.clone());
                        }
                    }

                    let mut error_msg = format!("Execution failed: {}", e);
                    if let Some(ref alt) = alternative_suggestion {
                        error_msg.push_str(&format!("\nSuggested alternative: {}", alt));
                    }

                    // ── Record experience ─────────────────────────────────────
                    if let Some(ref learning) = learning_engine {
                        let ctx = ExperienceContext::current(&plan_goal);
                        let _ = learning
                            .record_experience(
                                &action_name,
                                &task.action,
                                false,
                                Some(&error_str),
                                alternative_suggestion.as_deref(),
                                &ctx,
                            )
                            .await;
                    }

                    return TaskExecutionOutcome::Failure(error_msg);
                }
            }
        }
    }

    TaskExecutionOutcome::Failure("Exhausted all retries".to_string())
}

/// Resolve a [`DesktopAction`] into an [`ActionResult`] (standalone version
/// for concurrent execution).
async fn resolve_action_standalone(
    action: &crate::computer::DesktopAction,
    adapter: &Arc<dyn ComputerAdapter>,
    tool_registry: &Option<Arc<ToolRegistry>>,
) -> crate::Result<ActionResult> {
    match action {
        crate::computer::DesktopAction::ToolCall { tool_name, args } => {
            execute_tool_call_standalone(tool_name, args, tool_registry).await
        }
        other => adapter.execute(other.clone()).await.map_err(|e| {
            crate::error::SyscityError::ExternalService {
                source: e.to_string(),
                cause: None,
            }
        }),
    }
}

/// Execute a tool call through the [`ToolRegistry`] (standalone version
/// for concurrent execution).
async fn execute_tool_call_standalone(
    tool_name: &str,
    args: &serde_json::Value,
    tool_registry: &Option<Arc<ToolRegistry>>,
) -> crate::Result<ActionResult> {
    let registry = tool_registry.as_ref().ok_or_else(|| {
        crate::error::SyscityError::Validation(
            "ToolCall requires ToolRegistry but none is configured on the executor".to_string(),
        )
    })?;

    let context = crate::tools::ToolContext::default();
    match registry.execute(tool_name, args.clone(), &context).await {
        Some(Ok(result)) => Ok(ActionResult {
            success: result.success,
            message: result.output,
            screenshot_after: None,
            data: result.data,
        }),
        Some(Err(e)) => Err(e),
        None => Err(crate::error::SyscityError::NotFound {
            resource: format!("Tool '{}' referenced by ToolCall", tool_name),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::planner::{Plan, Task, TaskStatus};
    use crate::tools::{Tool, ToolContext, ToolExecutionResult, ToolRegistry};

    // ── Mock tool for ToolCall tests ──────────────────────────────────────

    struct MockDeviceReadTool;

    #[async_trait]
    impl Tool for MockDeviceReadTool {
        fn name(&self) -> &str {
            "mock_device_read"
        }

        fn description(&self) -> &str {
            "Mock device read tool"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({ "type": "object", "properties": {} })
        }

        async fn execute(
            &self,
            _args: serde_json::Value,
            _ctx: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            Ok(ToolExecutionResult {
                success: true,
                output: "read value: 42".to_string(),
                data: Some(json!({ "value": 42 })),
                error: None,
                execution_time: std::time::Duration::from_secs(0),
            })
        }
    }

    // ── Config default test ───────────────────────────────────────────────

    #[test]
    fn test_executor_config_default() {
        let cfg = ExecutorConfig::default();
        assert_eq!(cfg.max_concurrency, 4);
        assert!(cfg.enable_rollback);
    }

    // ── Cancelled controller test ─────────────────────────────────────────

    #[tokio::test]
    async fn test_executor_with_cancelled_controller() {
        let adapter = Arc::new(crate::computer::headless::HeadlessComputerAdapter::new());
        let verifier = VerificationEngine::new(adapter.clone());
        let ctrl = crate::acp::ExecutionController::new();
        ctrl.cancel().await;

        let executor = TaskExecutor::new(adapter, verifier).with_execution_controller(ctrl);

        let mut plan = Plan::new("test plan".to_string());
        plan.add_task(Task {
            id: "t1".to_string(),
            description: "noop".to_string(),
            action: crate::computer::DesktopAction::Wait { milliseconds: 0 },
            dependencies: vec![],
            max_retries: 0,
            retry_delay: std::time::Duration::from_millis(0),
            verification: None,
            snapshot_before: false,
            status: TaskStatus::Pending,
            error: None,
            result: None,
        });

        let result = executor.execute(&mut plan).await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("cancelled"));
    }

    // ── ToolCall tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_executor_tool_call_success() {
        let tool_registry = Arc::new(ToolRegistry::new());
        tool_registry.register_dynamic(Arc::new(MockDeviceReadTool));

        let adapter = Arc::new(crate::computer::headless::HeadlessComputerAdapter::new());
        let verifier = VerificationEngine::new(adapter.clone());
        let executor = TaskExecutor::new(adapter, verifier).with_tool_registry(tool_registry);

        let mut plan = Plan::new("tool call test".to_string());
        plan.add_task(Task {
            id: "read-sensor".to_string(),
            description: "Read sensor via tool call".to_string(),
            action: crate::computer::DesktopAction::ToolCall {
                tool_name: "mock_device_read".to_string(),
                args: json!({}),
            },
            dependencies: vec![],
            max_retries: 0,
            retry_delay: std::time::Duration::from_millis(0),
            verification: None,
            snapshot_before: false,
            status: TaskStatus::Pending,
            error: None,
            result: None,
        });

        let result = executor.execute(&mut plan).await.unwrap();
        assert!(result.success, "ToolCall plan should succeed: {}", result.message);
        assert_eq!(result.tasks_completed, 1);
        assert_eq!(result.tasks_failed, 0);
    }

    #[tokio::test]
    async fn test_executor_tool_call_no_registry() {
        let adapter = Arc::new(crate::computer::headless::HeadlessComputerAdapter::new());
        let verifier = VerificationEngine::new(adapter.clone());
        // No with_tool_registry() — ToolRegistry not configured
        let executor = TaskExecutor::new(adapter, verifier);

        let mut plan = Plan::new("tool call without registry".to_string());
        plan.add_task(Task {
            id: "call".to_string(),
            description: "Call tool without registry".to_string(),
            action: crate::computer::DesktopAction::ToolCall {
                tool_name: "any_tool".to_string(),
                args: json!({}),
            },
            dependencies: vec![],
            max_retries: 0,
            retry_delay: std::time::Duration::from_millis(0),
            verification: None,
            snapshot_before: false,
            status: TaskStatus::Pending,
            error: None,
            result: None,
        });

        let result = executor.execute(&mut plan).await.unwrap();
        assert!(!result.success, "Should fail without ToolRegistry");
        assert_eq!(result.tasks_failed, 1);
    }

    #[tokio::test]
    async fn test_executor_tool_call_not_found() {
        let tool_registry = Arc::new(ToolRegistry::new());
        // Register a tool but call a different one
        tool_registry.register_dynamic(Arc::new(MockDeviceReadTool));

        let adapter = Arc::new(crate::computer::headless::HeadlessComputerAdapter::new());
        let verifier = VerificationEngine::new(adapter.clone());
        let executor = TaskExecutor::new(adapter, verifier).with_tool_registry(tool_registry);

        let mut plan = Plan::new("tool call not found".to_string());
        plan.add_task(Task {
            id: "call".to_string(),
            description: "Call nonexistent tool".to_string(),
            action: crate::computer::DesktopAction::ToolCall {
                tool_name: "nonexistent_tool".to_string(),
                args: json!({}),
            },
            dependencies: vec![],
            max_retries: 0,
            retry_delay: std::time::Duration::from_millis(0),
            verification: None,
            snapshot_before: false,
            status: TaskStatus::Pending,
            error: None,
            result: None,
        });

        let result = executor.execute(&mut plan).await.unwrap();
        assert!(!result.success, "Should fail for nonexistent tool");
        assert_eq!(result.tasks_failed, 1);
    }

    #[test]
    fn test_desktop_action_name_tool_call() {
        let action = crate::computer::DesktopAction::ToolCall {
            tool_name: "test".to_string(),
            args: json!({}),
        };
        assert_eq!(desktop_action_name(&action), "tool_call");
    }
}
