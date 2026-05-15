//! Update Plan Tool — Agent Task Planning with Ordered Steps
//!
//! OpenClaw-compatible tool for creating and updating execution plans.
//! Each plan has ordered steps with status: pending, in_progress, or completed.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use super::{Tool, ToolContext, ToolExecutionResult};

/// Status of a plan step
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// A single step in a plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
    pub status: StepStatus,
    pub notes: Option<String>,
}

/// An execution plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub title: String,
    pub steps: Vec<PlanStep>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// In-memory plan store
#[derive(Debug, Clone, Default)]
pub struct PlanStore {
    plans: Arc<RwLock<HashMap<String, Plan>>>,
}

impl PlanStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self, plan_id: &str) -> Option<Plan> {
        let plans = self.plans.read().await;
        plans.get(plan_id).cloned()
    }

    pub async fn create(&self, plan: Plan) {
        let mut plans = self.plans.write().await;
        plans.insert(plan.id.clone(), plan);
    }

    pub async fn update(&self, plan: Plan) {
        let mut plans = self.plans.write().await;
        plans.insert(plan.id.clone(), plan);
    }

    pub async fn list(&self) -> Vec<Plan> {
        let plans = self.plans.read().await;
        plans.values().cloned().collect()
    }

    pub async fn delete(&self, plan_id: &str) {
        let mut plans = self.plans.write().await;
        plans.remove(plan_id);
    }
}

/// Tool for creating and updating execution plans
pub struct UpdatePlanTool {
    store: PlanStore,
}

impl UpdatePlanTool {
    pub fn new() -> Self {
        Self {
            store: PlanStore::new(),
        }
    }
}

impl Default for UpdatePlanTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
enum UpdatePlanAction {
    Create {
        title: String,
        #[serde(default)]
        steps: Vec<String>,
    },
    Update {
        plan_id: String,
        #[serde(default)]
        steps: Option<Vec<PlanStepArg>>,
    },
    Get {
        plan_id: String,
    },
    List,
    Delete {
        plan_id: String,
    },
    SetStatus {
        plan_id: String,
        step_id: String,
        status: StepStatus,
        #[serde(default)]
        notes: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct PlanStepArg {
    id: String,
    description: String,
    #[serde(default)]
    status: Option<StepStatus>,
}

#[async_trait]
impl Tool for UpdatePlanTool {
    fn name(&self) -> &str {
        "update_plan"
    }

    fn description(&self) -> &str {
        "Create, update, and manage execution plans with ordered steps. \
         Supports creating plans, updating step statuses (pending/in_progress/completed/failed), \
         and retrieving plan state."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "update", "get", "list", "delete", "set_status"],
                    "description": "Action to perform"
                },
                "plan_id": {
                    "type": "string",
                    "description": "Plan ID (required for update, get, delete, set_status)"
                },
                "title": {
                    "type": "string",
                    "description": "Plan title (required for create)"
                },
                "steps": {
                    "type": "array",
                    "description": "List of step descriptions (for create) or step objects (for update)",
                    "items": {
                        "oneOf": [
                            { "type": "string" },
                            {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "description": { "type": "string" },
                                    "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "failed"] }
                                },
                                "required": ["id", "description"]
                            }
                        ]
                    }
                },
                "step_id": {
                    "type": "string",
                    "description": "Step ID (required for set_status)"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "failed"],
                    "description": "New status (required for set_status)"
                },
                "notes": {
                    "type": "string",
                    "description": "Optional notes for set_status"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let action: UpdatePlanAction = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Invalid arguments: {}", e)),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
        };

        match action {
            UpdatePlanAction::Create { title, steps } => {
                let plan_id = uuid::Uuid::new_v4().to_string();
                let now = chrono::Utc::now();
                let plan_steps: Vec<PlanStep> = steps
                    .into_iter()
                    .enumerate()
                    .map(|(i, desc)| PlanStep {
                        id: format!("step_{}", i + 1),
                        description: desc,
                        status: StepStatus::Pending,
                        notes: None,
                    })
                    .collect();

                let plan = Plan {
                    id: plan_id.clone(),
                    title,
                    steps: plan_steps,
                    created_at: now,
                    updated_at: now,
                };

                self.store.create(plan.clone()).await;
                info!("Created plan {} with {} step(s)", plan_id, plan.steps.len());

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Created plan '{}' with {} step(s)", plan.title, plan.steps.len()),
                    error: None,
                    data: Some(serde_json::to_value(plan).unwrap_or_default()),
                    execution_time: start.elapsed(),
                })
            }
            UpdatePlanAction::Update { plan_id, steps } => {
                let mut plan = match self.store.get(&plan_id).await {
                    Some(p) => p,
                    None => {
                        return Ok(ToolExecutionResult {
                            success: false,
                            output: String::new(),
                            error: Some(format!("Plan {} not found", plan_id)),
                            data: None,
                            execution_time: start.elapsed(),
                        });
                    }
                };

                if let Some(new_steps) = steps {
                    plan.steps = new_steps
                        .into_iter()
                        .map(|s| PlanStep {
                            id: s.id,
                            description: s.description,
                            status: s.status.unwrap_or(StepStatus::Pending),
                            notes: None,
                        })
                        .collect();
                }
                plan.updated_at = chrono::Utc::now();

                self.store.update(plan.clone()).await;

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Updated plan '{}' with {} step(s)", plan.title, plan.steps.len()),
                    error: None,
                    data: Some(serde_json::to_value(plan).unwrap_or_default()),
                    execution_time: start.elapsed(),
                })
            }
            UpdatePlanAction::Get { plan_id } => {
                match self.store.get(&plan_id).await {
                    Some(plan) => Ok(ToolExecutionResult {
                        success: true,
                        output: format!("Plan '{}': {} step(s)", plan.title, plan.steps.len()),
                        error: None,
                        data: Some(serde_json::to_value(plan).unwrap_or_default()),
                        execution_time: start.elapsed(),
                    }),
                    None => Ok(ToolExecutionResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Plan {} not found", plan_id)),
                        data: None,
                        execution_time: start.elapsed(),
                    }),
                }
            }
            UpdatePlanAction::List => {
                let plans = self.store.list().await;
                let summary: Vec<_> = plans
                    .iter()
                    .map(|p| {
                        serde_json::json!({
                            "id": p.id,
                            "title": p.title,
                            "step_count": p.steps.len(),
                            "completed": p.steps.iter().filter(|s| s.status == StepStatus::Completed).count(),
                        })
                    })
                    .collect();

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("{} plan(s) found", plans.len()),
                    error: None,
                    data: Some(serde_json::json!({ "plans": summary })),
                    execution_time: start.elapsed(),
                })
            }
            UpdatePlanAction::Delete { plan_id } => {
                self.store.delete(&plan_id).await;
                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Plan {} deleted", plan_id),
                    error: None,
                    data: None,
                    execution_time: start.elapsed(),
                })
            }
            UpdatePlanAction::SetStatus {
                plan_id,
                step_id,
                status,
                notes,
            } => {
                let mut plan = match self.store.get(&plan_id).await {
                    Some(p) => p,
                    None => {
                        return Ok(ToolExecutionResult {
                            success: false,
                            output: String::new(),
                            error: Some(format!("Plan {} not found", plan_id)),
                            data: None,
                            execution_time: start.elapsed(),
                        });
                    }
                };

                let mut found = false;
                for step in &mut plan.steps {
                    if step.id == step_id {
                        step.status = status;
                        if notes.is_some() {
                            step.notes = notes;
                        }
                        found = true;
                        break;
                    }
                }

                if !found {
                    return Ok(ToolExecutionResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Step {} not found in plan {}", step_id, plan_id)),
                        data: None,
                        execution_time: start.elapsed(),
                    });
                }

                plan.updated_at = chrono::Utc::now();
                self.store.update(plan.clone()).await;

                let completed = plan.steps.iter().filter(|s| s.status == StepStatus::Completed).count();
                let total = plan.steps.len();

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!(
                        "Step {} set to {:?}. Progress: {}/{} completed",
                        step_id, status, completed, total
                    ),
                    error: None,
                    data: Some(serde_json::to_value(plan).unwrap_or_default()),
                    execution_time: start.elapsed(),
                })
            }
        }
    }
}
