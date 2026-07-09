//! Tool wrapper around [`GoalPlanner`] exposing multi-step goal decomposition
//! and execution as a standard Tool.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::planner::GoalPlanner;
use crate::tools::{
    approval::RiskLevel, create_schema, sdk::ToolCapabilities, Tool, ToolContext,
    ToolExecutionResult,
};

/// Tool that exposes the [`GoalPlanner`] to the LLM via standard tool calling.
///
/// The planner decomposes complex goals into dependency-aware DAG plans and
/// executes them step by step.  The LLM decides when to invoke this tool
/// rather than being pre-routed by heuristics.
///
/// The planner reference (`Arc<GoalPlanner>`) is set during agent spawn via a
/// shared [`RwLock`] handle so the tool registry can stay agent-agnostic.
pub struct PlannerTool {
    planner_handle: Arc<RwLock<Option<Arc<GoalPlanner>>>>,
}

impl PlannerTool {
    pub fn new(planner_handle: Arc<RwLock<Option<Arc<GoalPlanner>>>>) -> Self {
        Self { planner_handle }
    }
}

#[async_trait]
impl Tool for PlannerTool {
    fn name(&self) -> &str {
        "planner"
    }

    fn description(&self) -> &str {
        r#"Decompose complex goals into multi-step, dependency-aware plans and execute them.

Use this tool when a task requires multiple coordinated steps — for example:
deploying software, running a multi-stage build, configuring a system,
installing packages with verification, migrating data, or any workflow where
steps depend on each other and may need retries or rollback.

The planner handles task decomposition, dependency ordering, parallel execution
of independent tasks, automatic retries, and rollback on failure. Returns a
summary of completed, failed, and rolled-back tasks.

For simple single-step operations, use the relevant individual tool instead."#
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Multi-step goal planning and execution",
            json!({
                "goal": {
                    "type": "string",
                    "description": "The high-level goal to achieve (e.g. 'deploy the web app to staging')"
                },
                "max_tasks": {
                    "type": "integer",
                    "description": "Maximum number of tasks to decompose the goal into (default: auto)"
                }
            }),
            vec!["goal"],
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: true,
            risk_level: RiskLevel::High,
            categories: vec!["planner".to_string(), "orchestration".to_string()],
            ..Default::default()
        }
    }

    fn is_available(&self, _context: &ToolContext) -> bool {
        self.planner_handle
            .read()
            .map(|p| p.is_some())
            .unwrap_or(false)
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let goal = args["goal"].as_str().ok_or_else(|| {
            crate::error::SyscityError::Validation(
                "Missing 'goal' argument for planner tool".to_string(),
            )
        })?;

        let planner = {
            let guard = self.planner_handle.read().map_err(|e| {
                crate::error::SyscityError::Internal(format!("Planner handle poisoned: {}", e))
            })?;
            guard.clone()
        };
        let planner = planner.ok_or_else(|| {
            crate::error::SyscityError::Unsupported(
                "Planner is not available (no adapter configured)".to_string(),
            )
        })?;

        // The planner's executor already has its own ToolRegistry reference;
        // we pass an empty tool list here since the decomposer hint is optional.
        let available_tools: Vec<String> = vec![];

        let result = planner.achieve(goal, &available_tools).await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("GoalPlanner failed: {}", e))
        })?;

        let summary = format!(
            "Goal: {}\nSuccess: {}\nCompleted: {}, Failed: {}, Rolled back: {}\n\n{}",
            result.goal,
            if result.success { "Yes" } else { "No" },
            result.tasks_completed,
            result.tasks_failed,
            result.tasks_rolled_back,
            result.message
        );

        let data = json!({
            "success": result.success,
            "goal": result.goal,
            "tasks_completed": result.tasks_completed,
            "tasks_failed": result.tasks_failed,
            "tasks_rolled_back": result.tasks_rolled_back,
        });

        Ok(ToolExecutionResult::success(summary).with_data(data))
    }
}
