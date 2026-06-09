//! Flow execution engine - a recoverable approval workflow runtime
//!
//! This module provides a DAG-based workflow execution engine with support for
//! approval steps, pause/resume, cancellation, and recovery of interrupted flows.
//! Steps are executed in topological order according to their dependency graph.
//!
//! # Overview
//!
//! - [`FlowEngine`] is the main entry point for creating and executing flows.
//! - [`FlowStore`] is a storage trait that can be implemented for persistence.
//! - [`InMemoryFlowStore`] provides an in-memory implementation for testing.
//!
//! # Example
//!
//! ```rust,ignore
//! use syscity::flow::*;
//!
//! let store = Arc::new(InMemoryFlowStore::new()) as Arc<dyn FlowStore>;
//! let engine = FlowEngine::new(store);
//!
//! let steps = vec![
//!     FlowStep {
//!         id: FlowStepId("step1".into()),
//!         name: "Step 1".into(),
//!         description: "First step".into(),
//!         tool_name: "example_tool".into(),
//!         tool_args: serde_json::json!({"key": "value"}),
//!         depends_on: vec![],
//!         approval: ApprovalRequirement::Never,
//!         timeout_secs: 60,
//!         retry_count: 0,
//!         on_failure: FailureAction::Abort,
//!     },
//! ];
//!
//! let flow = Flow::new("example", steps);
//! let flow_id = engine.create_flow(flow).await.unwrap();
//! let result = engine.execute_flow(&flow_id).await.unwrap();
//! ```

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Newtypes
// ---------------------------------------------------------------------------

/// Unique identifier for a flow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct FlowId(pub String);

/// Unique identifier for a step within a flow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct FlowStepId(pub String);

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Describes when a step requires human approval before execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalRequirement {
    /// Approval is never required; the step runs automatically.
    Never,
    /// Approval is always required before this step can execute.
    Always,
    /// Approval is required and collected after all prerequisite steps finish.
    AfterAll,
}

/// Action to take when a step fails.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FailureAction {
    /// Abort the entire flow immediately.
    Abort,
    /// Skip this step and continue with dependents.
    Skip,
    /// Retry the step (up to its configured retry count).
    Retry,
    /// Continue as if the step succeeded (best-effort).
    Continue,
}

/// Overall status of a flow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FlowStatus {
    /// Flow has been created but not yet started.
    Pending,
    /// Flow is currently executing steps.
    Running,
    /// Flow execution has been paused by the user.
    Paused,
    /// All steps completed successfully.
    Completed,
    /// One or more steps failed and the flow was aborted.
    Failed,
    /// Flow was cancelled by the user.
    Cancelled,
}

/// Status of an individual step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StepStatus {
    /// Step has not started yet.
    Pending,
    /// Step is currently executing.
    Running,
    /// Step completed successfully.
    Succeeded,
    /// Step failed during execution.
    Failed,
    /// Step was skipped (e.g., due to a failure action of Skip).
    Skipped,
    /// Step is waiting for human approval.
    WaitingApproval,
    /// Human approval has been granted; step is ready to execute.
    Approved,
    /// Human approval was denied.
    Rejected,
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Definition of a single step in a flow DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowStep {
    /// Unique identifier for this step.
    pub id: FlowStepId,
    /// Human-readable name.
    pub name: String,
    /// Longer description of what this step does.
    pub description: String,
    /// Name of the tool/function to invoke.
    pub tool_name: String,
    /// Arguments to pass to the tool, as a JSON value.
    pub tool_args: serde_json::Value,
    /// Steps that must complete before this step runs.
    pub depends_on: Vec<FlowStepId>,
    /// Approval requirements for this step.
    pub approval: ApprovalRequirement,
    /// Maximum time in seconds to wait for step completion.
    pub timeout_secs: u64,
    /// Number of times to retry on failure (if `on_failure` is `Retry`).
    pub retry_count: u32,
    /// Action to take if this step fails.
    pub on_failure: FailureAction,
}

/// Runtime state for a single step execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepExecutionState {
    /// Step identifier (matches the corresponding `FlowStep.id`).
    pub id: FlowStepId,
    /// Current status of this step.
    pub status: StepStatus,
    /// Input arguments provided to the step.
    pub input: serde_json::Value,
    /// Output produced by the step.
    pub output: serde_json::Value,
    /// Error message if the step failed.
    pub error: Option<String>,
    /// Number of execution attempts so far.
    pub attempts: u32,
    /// When the step started executing.
    pub started_at: Option<DateTime<Utc>>,
    /// When the step completed execution.
    pub completed_at: Option<DateTime<Utc>>,
}

/// Full runtime state for a flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowState {
    /// Flow identifier.
    pub flow_id: FlowId,
    /// The flow definition (static structure).
    pub flow_definition: Flow,
    /// Per-step execution states, keyed by step ID.
    pub step_states: HashMap<FlowStepId, StepExecutionState>,
    /// Current flow status.
    pub status: FlowStatus,
    /// When the flow was created.
    pub created_at: DateTime<Utc>,
    /// When the flow was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Defines a flow's structure (the DAG metadata).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flow {
    /// Unique identifier for this flow.
    pub id: FlowId,
    /// Human-readable name.
    pub name: String,
    /// Ordered list of steps. The actual execution order is determined
    /// by topological sort of their dependency graph.
    pub steps: Vec<FlowStep>,
    /// Maximum number of ready steps that may execute concurrently.
    pub max_concurrency: usize,
    /// Whether to automatically clean up flow state after completion.
    pub auto_cleanup: bool,
}

/// Summary result after a flow finishes execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowResult {
    /// Flow identifier.
    pub flow_id: FlowId,
    /// Human-readable name.
    pub flow_name: String,
    /// Final status of the flow.
    pub status: FlowStatus,
    /// Total number of steps in the flow.
    pub total_steps: usize,
    /// Number of steps that succeeded.
    pub succeeded: usize,
    /// Number of steps that failed.
    pub failed: usize,
    /// Number of steps that were skipped.
    pub skipped: usize,
    /// Per-step execution results.
    pub step_results: HashMap<FlowStepId, StepExecutionState>,
}

/// Lightweight summary for listing flows without loading full state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowSummary {
    /// Flow identifier.
    pub flow_id: FlowId,
    /// Human-readable name.
    pub name: String,
    /// Current flow status.
    pub status: FlowStatus,
    /// When the flow was created.
    pub created_at: DateTime<Utc>,
    /// When the flow was last updated.
    pub updated_at: DateTime<Utc>,
    /// Number of steps in the flow.
    pub total_steps: usize,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during flow operations.
#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    /// The requested flow was not found.
    #[error("Flow not found: {0}")]
    NotFound(String),
    /// The flow is already in a terminal state and cannot be acted upon.
    #[error("Flow already completed: {0}")]
    AlreadyCompleted(String),
    /// The DAG contains a cycle or has another structural problem.
    #[error("Invalid DAG: {0}")]
    InvalidDag(String),
    /// A storage-level error occurred.
    #[error("Storage error: {0}")]
    Storage(String),
}

/// Alias for `Result` using `FlowError`.
pub type Result<T> = std::result::Result<T, FlowError>;

// ---------------------------------------------------------------------------
// FlowStore trait
// ---------------------------------------------------------------------------

/// Persistence interface for flow state.
///
/// Implementations must be `Send + Sync` so they can be shared across async
/// tasks via `Arc<dyn FlowStore>`.
#[async_trait]
pub trait FlowStore: Send + Sync {
    /// Persist (create or update) a flow's state.
    async fn save_flow(&self, state: &FlowState) -> Result<()>;
    /// Load a flow's full state by its identifier.
    async fn load_flow(&self, flow_id: &FlowId) -> Result<Option<FlowState>>;
    /// Delete a flow's state from storage.
    async fn delete_flow(&self, flow_id: &FlowId) -> Result<()>;
    /// Return a summary list of all known flows.
    async fn list_flows(&self) -> Result<Vec<FlowSummary>>;
}

// ---------------------------------------------------------------------------
// InMemoryFlowStore
// ---------------------------------------------------------------------------

/// In-memory implementation of [`FlowStore`].
///
/// Flows are stored in an `Arc<RwLock<HashMap>>`, making this suitable for
/// testing and single-node deployments without persistence.
pub struct InMemoryFlowStore {
    flows: Arc<RwLock<HashMap<String, FlowState>>>,
}

impl InMemoryFlowStore {
    /// Create a new, empty in-memory store.
    pub fn new() -> Self {
        Self {
            flows: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryFlowStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FlowStore for InMemoryFlowStore {
    async fn save_flow(&self, state: &FlowState) -> Result<()> {
        let mut map = self.flows.write().await;
        map.insert(state.flow_id.0.clone(), state.clone());
        Ok(())
    }

    async fn load_flow(&self, flow_id: &FlowId) -> Result<Option<FlowState>> {
        let map = self.flows.read().await;
        Ok(map.get(&flow_id.0).cloned())
    }

    async fn delete_flow(&self, flow_id: &FlowId) -> Result<()> {
        let mut map = self.flows.write().await;
        map.remove(&flow_id.0);
        Ok(())
    }

    async fn list_flows(&self) -> Result<Vec<FlowSummary>> {
        let map = self.flows.read().await;
        Ok(map
            .values()
            .map(|s| FlowSummary {
                flow_id: s.flow_id.clone(),
                name: s.flow_definition.name.clone(),
                status: s.status.clone(),
                created_at: s.created_at,
                updated_at: s.updated_at,
                total_steps: s.flow_definition.steps.len(),
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// FlowEngine
// ---------------------------------------------------------------------------

/// The main workflow execution engine.
///
/// `FlowEngine` is responsible for creating flows, executing them via
/// topological ordering of their step DAG, managing approval states, and
/// providing pause/resume/cancel lifecycle controls.
pub struct FlowEngine {
    store: Arc<dyn FlowStore>,
}

impl FlowEngine {
    /// Create a new engine backed by the given [`FlowStore`].
    pub fn new(store: Arc<dyn FlowStore>) -> Self {
        Self { store }
    }

    /// Generate a unique [`FlowId`] (UUID v4).
    fn generate_id() -> FlowId {
        FlowId(Uuid::new_v4().to_string())
    }

    /// Create a new flow and persist its initial state.
    ///
    /// The flow is assigned a unique identifier and stored with `Pending`
    /// status. The returned [`FlowId`] can be used with other engine methods.
    pub async fn create_flow(&self, mut flow: Flow) -> Result<FlowId> {
        let flow_id = Self::generate_id();
        flow.id = flow_id.clone();

        let now = Utc::now();
        let step_states: HashMap<FlowStepId, StepExecutionState> = flow
            .steps
            .iter()
            .map(|step| {
                let state = StepExecutionState {
                    id: step.id.clone(),
                    status: StepStatus::Pending,
                    input: step.tool_args.clone(),
                    output: serde_json::Value::Null,
                    error: None,
                    attempts: 0,
                    started_at: None,
                    completed_at: None,
                };
                (step.id.clone(), state)
            })
            .collect();

        let state = FlowState {
            flow_id: flow_id.clone(),
            flow_definition: flow,
            step_states,
            status: FlowStatus::Pending,
            created_at: now,
            updated_at: now,
        };

        self.store.save_flow(&state).await?;
        Ok(flow_id)
    }

    /// Execute (or resume executing) a flow.
    ///
    /// The flow's steps are executed in topological order. If a step requires
    /// approval, it is set to `WaitingApproval` and the engine pauses
    /// execution (the flow stays `Running`). Call [`approve_step`](Self::approve_step)
    /// followed by [`resume_flow`](Self::resume_flow) to continue.
    ///
    /// Returns a [`FlowResult`] summarizing the outcome.
    pub async fn execute_flow(&self, flow_id: &FlowId) -> Result<FlowResult> {
        let mut state = self
            .store
            .load_flow(flow_id)
            .await?
            .ok_or_else(|| FlowError::NotFound(flow_id.0.clone()))?;

        // Refuse to re-execute flows that are already in a terminal state.
        if matches!(
            state.status,
            FlowStatus::Completed | FlowStatus::Failed | FlowStatus::Cancelled
        ) {
            return Err(FlowError::AlreadyCompleted(flow_id.0.clone()));
        }

        state.status = FlowStatus::Running;
        state.updated_at = Utc::now();
        self.store.save_flow(&state).await?;

        // Validate the DAG via topological sort (returns owned IDs to avoid
        // borrow conflicts when mutating state).
        let sorted_ids = topological_sort_ids(&state.flow_definition.steps)?;

        // Run the execution loop.
        self.execute_inner(&mut state, &sorted_ids).await?;

        // Determine final status after the loop exits.
        let all_done = state
            .step_states
            .values()
            .all(|s| matches!(s.status, StepStatus::Succeeded | StepStatus::Failed | StepStatus::Skipped | StepStatus::Rejected));
        let all_succeeded = state
            .step_states
            .values()
            .all(|s| matches!(s.status, StepStatus::Succeeded | StepStatus::Skipped));
        let has_waiting_approval = state
            .step_states
            .values()
            .any(|s| matches!(s.status, StepStatus::WaitingApproval));

        if state.status == FlowStatus::Cancelled {
            // Already cancelled by cancel_flow; leave as-is.
        } else if state.status == FlowStatus::Paused {
            // Paused by pause_flow; leave as-is.
        } else if has_waiting_approval {
            // Leave as Running -- waiting for external approval.
        } else if all_succeeded {
            state.status = FlowStatus::Completed;
        } else if all_done {
            state.status = FlowStatus::Failed;
        }
        // If not all done and not waiting for approval, something is wrong --
        // but we leave the status as Running so the caller can inspect.

        state.updated_at = Utc::now();
        self.store.save_flow(&state).await?;

        Ok(self.build_result(&state))
    }

    /// Inner execution loop.
    ///
    /// Repeatedly finds ready steps (those whose dependencies have all
    /// succeeded and whose own status is `Pending` or `Approved`), executes
    /// them, and saves state after each batch.  Exits when:
    /// - The flow is paused or cancelled.
    /// - No ready steps remain and all steps are either done or waiting for
    ///   approval.
    async fn execute_inner(
        &self,
        state: &mut FlowState,
        sorted_ids: &[FlowStepId],
    ) -> Result<()> {
        loop {
            // Respect pause / cancel signals.
            if state.status == FlowStatus::Paused || state.status == FlowStatus::Cancelled {
                break;
            }

            let ready_ids = self.find_ready_step_ids(state, sorted_ids);

            if ready_ids.is_empty() {
                // No steps are ready right now. Check whether we are done or
                // stuck waiting for approval.
                let all_done = state.step_states.values().all(|s| {
                    matches!(
                        s.status,
                        StepStatus::Succeeded
                            | StepStatus::Failed
                            | StepStatus::Skipped
                            | StepStatus::Rejected
                    )
                });
                let waiting_for_approval = state
                    .step_states
                    .values()
                    .any(|s| matches!(s.status, StepStatus::WaitingApproval));

                if all_done || waiting_for_approval {
                    break;
                }

                // No ready steps, not done, and not waiting -- this indicates
                // an unresolvable DAG state.
                return Err(FlowError::InvalidDag(
                    "No ready steps but not all steps are done. \
                     The DAG may have unsatisfiable dependencies."
                        .to_string(),
                ));
            }

            // Execute ready steps (subject to the concurrency limit).
            for step_id in ready_ids.iter().take(state.flow_definition.max_concurrency.max(1)) {
                self.execute_step(state, step_id).await?;

                if state.status == FlowStatus::Cancelled {
                    break;
                }
            }

            // Persist after each batch.
            state.updated_at = Utc::now();
            self.store.save_flow(state).await?;
        }

        Ok(())
    }

    /// Find steps that are ready to execute.
    ///
    /// A step is ready when:
    /// - Its status is `Pending` or `Approved`.
    /// - All steps it depends on have status `Succeeded` or `Skipped`.
    fn find_ready_step_ids(&self, state: &FlowState, sorted_ids: &[FlowStepId]) -> Vec<FlowStepId> {
        sorted_ids
            .iter()
            .filter(|step_id| {
                let step_state = match state.step_states.get(step_id) {
                    Some(s) => s,
                    None => return false,
                };

                // Must be Pending (not yet processed) or Approved (granted).
                if !matches!(step_state.status, StepStatus::Pending | StepStatus::Approved) {
                    return false;
                }

                // Look up the step definition to check dependencies.
                let step_def = match state.flow_definition.steps.iter().find(|s| &s.id == *step_id)
                {
                    Some(s) => s,
                    None => return false,
                };

                // All dependencies must have succeeded or been skipped.
                step_def.depends_on.iter().all(|dep_id| {
                    state
                        .step_states
                        .get(dep_id)
                        .map(|s| {
                            matches!(s.status, StepStatus::Succeeded | StepStatus::Skipped)
                        })
                        .unwrap_or(false)
                })
            })
            .cloned()
            .collect()
    }

    /// Execute (or stall for approval) a single step.
    ///
    /// If the step is `Pending` and requires approval, it is moved to
    /// `WaitingApproval` and execution returns immediately.
    ///
    /// Otherwise (the step is `Approved`, or `Pending` with no approval
    /// requirement), the step is simulated: its status becomes `Succeeded`
    /// and a mock output is recorded.
    async fn execute_step(&self, state: &mut FlowState, step_id: &FlowStepId) -> Result<()> {
        let step_state = state
            .step_states
            .get_mut(step_id)
            .unwrap_or_else(|| panic!("Step state must exist for {step_id:?}"));

        // Look up the step definition for metadata (approval, name, tool).
        let step_def = state
            .flow_definition
            .steps
            .iter()
            .find(|s| &s.id == step_id)
            .unwrap_or_else(|| panic!("Step definition must exist for {step_id:?}"));

        // --- Approval gate ---
        if step_state.status == StepStatus::Pending
            && matches!(
                step_def.approval,
                ApprovalRequirement::Always | ApprovalRequirement::AfterAll
            )
        {
            step_state.status = StepStatus::WaitingApproval;
            return Ok(());
        }

        // --- Execute (simulated) ---
        step_state.status = StepStatus::Running;
        step_state.attempts += 1;
        step_state.started_at = Some(Utc::now());

        // Simulated tool execution: always succeeds.
        step_state.status = StepStatus::Succeeded;
        step_state.output = serde_json::json!({
            "status": "completed",
            "step": step_def.name,
            "tool": step_def.tool_name,
        });
        step_state.completed_at = Some(Utc::now());

        Ok(())
    }

    /// Pause a running flow.
    ///
    /// The flow must be in `Running` status.  It is set to `Paused` and
    /// persisted; the execution loop will exit on the next iteration.
    pub async fn pause_flow(&self, flow_id: &FlowId) -> Result<()> {
        let mut state = self
            .store
            .load_flow(flow_id)
            .await?
            .ok_or_else(|| FlowError::NotFound(flow_id.0.clone()))?;

        if state.status != FlowStatus::Running {
            return Err(FlowError::AlreadyCompleted(format!(
                "Flow {} is not running (status: {:?})",
                flow_id.0, state.status
            )));
        }

        state.status = FlowStatus::Paused;
        state.updated_at = Utc::now();
        self.store.save_flow(&state).await
    }

    /// Resume a paused flow.
    ///
    /// The flow must be in `Paused` status.  It is set back to `Running` and
    /// the execution loop is re-entered.
    pub async fn resume_flow(&self, flow_id: &FlowId) -> Result<FlowResult> {
        let mut state = self
            .store
            .load_flow(flow_id)
            .await?
            .ok_or_else(|| FlowError::NotFound(flow_id.0.clone()))?;

        if state.status != FlowStatus::Paused {
            return Err(FlowError::AlreadyCompleted(format!(
                "Flow {} is not paused (status: {:?})",
                flow_id.0, state.status
            )));
        }

        state.status = FlowStatus::Running;
        state.updated_at = Utc::now();
        self.store.save_flow(&state).await?;

        // Re-enter the execution loop by delegating to execute_flow.
        self.execute_flow(flow_id).await
    }

    /// Cancel a flow.
    ///
    /// The flow is set to `Cancelled` status.  If the flow is currently
    /// executing, the execution loop will exit on the next iteration.
    pub async fn cancel_flow(&self, flow_id: &FlowId) -> Result<()> {
        let mut state = self
            .store
            .load_flow(flow_id)
            .await?
            .ok_or_else(|| FlowError::NotFound(flow_id.0.clone()))?;

        if matches!(
            state.status,
            FlowStatus::Completed | FlowStatus::Cancelled | FlowStatus::Failed
        ) {
            return Err(FlowError::AlreadyCompleted(format!(
                "Flow {} is already in terminal state ({:?})",
                flow_id.0, state.status
            )));
        }

        state.status = FlowStatus::Cancelled;
        state.updated_at = Utc::now();
        self.store.save_flow(&state).await
    }

    /// Mark a step as approved.
    ///
    /// The step must currently be in `WaitingApproval` status.  After
    /// approval, the next call to [`execute_flow`](Self::execute_flow) or
    /// [`resume_flow`](Self::resume_flow) will execute the step.
    pub async fn approve_step(&self, flow_id: &FlowId, step_id: &FlowStepId) -> Result<()> {
        let mut state = self
            .store
            .load_flow(flow_id)
            .await?
            .ok_or_else(|| FlowError::NotFound(flow_id.0.clone()))?;

        let step_state = state.step_states.get_mut(step_id).ok_or_else(|| {
            FlowError::NotFound(format!("Step {} not found in flow {}", step_id.0, flow_id.0))
        })?;

        if step_state.status != StepStatus::WaitingApproval {
            return Err(FlowError::InvalidDag(format!(
                "Step {} is not waiting for approval (status: {:?})",
                step_id.0, step_state.status
            )));
        }

        step_state.status = StepStatus::Approved;
        state.updated_at = Utc::now();
        self.store.save_flow(&state).await
    }

    /// Mark a step as rejected.
    ///
    /// The step must currently be in `WaitingApproval` status.  Rejected
    /// steps are treated as failures in the flow result.
    pub async fn reject_step(&self, flow_id: &FlowId, step_id: &FlowStepId) -> Result<()> {
        let mut state = self
            .store
            .load_flow(flow_id)
            .await?
            .ok_or_else(|| FlowError::NotFound(flow_id.0.clone()))?;

        let step_state = state.step_states.get_mut(step_id).ok_or_else(|| {
            FlowError::NotFound(format!("Step {} not found in flow {}", step_id.0, flow_id.0))
        })?;

        if step_state.status != StepStatus::WaitingApproval {
            return Err(FlowError::InvalidDag(format!(
                "Step {} is not waiting for approval (status: {:?})",
                step_id.0, step_state.status
            )));
        }

        step_state.status = StepStatus::Rejected;
        state.updated_at = Utc::now();
        self.store.save_flow(&state).await
    }

    /// Retrieve the full state of a flow, or `None` if it does not exist.
    pub async fn get_flow_state(&self, flow_id: &FlowId) -> Option<FlowState> {
        self.store.load_flow(flow_id).await.ok().flatten()
    }

    /// List all flows as lightweight summaries.
    pub async fn list_flows(&self) -> Result<Vec<FlowSummary>> {
        self.store.list_flows().await
    }

    /// Find flows that were interrupted (status `Running`).
    ///
    /// This is useful at startup to identify flows that were in the middle of
    /// execution when the process was shut down.
    pub async fn recover_interrupted_flows(&self) -> Result<Vec<FlowId>> {
        let flows = self.store.list_flows().await?;
        Ok(flows
            .into_iter()
            .filter(|s| s.status == FlowStatus::Running)
            .map(|s| s.flow_id)
            .collect())
    }

    /// Build a [`FlowResult`] summary from the current [`FlowState`].
    fn build_result(&self, state: &FlowState) -> FlowResult {
        let mut succeeded = 0;
        let mut failed = 0;
        let mut skipped = 0;

        for step_state in state.step_states.values() {
            match step_state.status {
                StepStatus::Succeeded => succeeded += 1,
                StepStatus::Failed | StepStatus::Rejected => failed += 1,
                StepStatus::Skipped => skipped += 1,
                _ => {}
            }
        }

        FlowResult {
            flow_id: state.flow_id.clone(),
            flow_name: state.flow_definition.name.clone(),
            status: state.status.clone(),
            total_steps: state.flow_definition.steps.len(),
            succeeded,
            failed,
            skipped,
            step_results: state.step_states.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Topological Sort (Kahn's algorithm)
// ---------------------------------------------------------------------------

/// Perform a topological sort on the flow's step DAG using Kahn's algorithm.
///
/// Returns the step IDs in execution order. Returns an error (with the string
/// `"Cycle detected"`) if the dependency graph contains a cycle.
fn topological_sort_ids(steps: &[FlowStep]) -> Result<Vec<FlowStepId>> {
    let mut in_degree: HashMap<&FlowStepId, usize> = HashMap::new();
    let mut adjacency: HashMap<&FlowStepId, Vec<&FlowStepId>> = HashMap::new();

    // Initialise data structures.
    for step in steps {
        in_degree.insert(&step.id, 0);
        adjacency.insert(&step.id, Vec::new());
    }

    // Build the graph: for each dependency edge `dep -> step.id`,
    // increment step's in-degree and add step.id to dep's adjacency list.
    for step in steps {
        for dep in &step.depends_on {
            if let Some(neighbors) = adjacency.get_mut(dep) {
                neighbors.push(&step.id);
            }
            if let Some(degree) = in_degree.get_mut(&step.id) {
                *degree += 1;
            }
        }
    }

    // Seeds: all nodes with in-degree 0.
    let mut queue: VecDeque<&FlowStepId> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(id, _)| *id)
        .collect();

    let mut sorted: Vec<FlowStepId> = Vec::with_capacity(steps.len());

    while let Some(id) = queue.pop_front() {
        sorted.push((*id).clone());

        if let Some(neighbors) = adjacency.get(id) {
            for neighbor in neighbors {
                if let Some(degree) = in_degree.get_mut(neighbor) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(neighbor);
                    }
                }
            }
        }
    }

    if sorted.len() != steps.len() {
        return Err(FlowError::InvalidDag(
            "Cycle detected in flow dependencies".to_string(),
        ));
    }

    Ok(sorted)
}

// ---------------------------------------------------------------------------
// Flow helpers
// ---------------------------------------------------------------------------

impl Flow {
    /// Create a new flow with the given name and steps.
    ///
    /// The flow's `id` is a placeholder (empty string) and will be assigned
    /// by [`FlowEngine::create_flow`].
    pub fn new(name: &str, steps: Vec<FlowStep>) -> Self {
        Self {
            id: FlowId(String::new()),
            name: name.to_string(),
            steps,
            max_concurrency: 1,
            auto_cleanup: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- helpers -----------------------------------------------------------

    fn make_step(id: &str, deps: Vec<&str>, approval: ApprovalRequirement) -> FlowStep {
        FlowStep {
            id: FlowStepId(id.to_string()),
            name: format!("Step {id}"),
            description: format!("Description for {id}"),
            tool_name: "test_tool".to_string(),
            tool_args: serde_json::json!({"key": id}),
            depends_on: deps.into_iter().map(|d| FlowStepId(d.to_string())).collect(),
            approval,
            timeout_secs: 60,
            retry_count: 0,
            on_failure: FailureAction::Abort,
        }
    }

    fn new_engine() -> FlowEngine {
        let store = Arc::new(InMemoryFlowStore::new()) as Arc<dyn FlowStore>;
        FlowEngine::new(store)
    }

    // -- tests -------------------------------------------------------------

    #[tokio::test]
    async fn test_create_flow_returns_valid_id() {
        let engine = new_engine();

        let steps = vec![make_step("s1", vec![], ApprovalRequirement::Never)];
        let flow = Flow::new("create_test", steps);
        let flow_id = engine.create_flow(flow).await.unwrap();

        assert!(!flow_id.0.is_empty(), "FlowId should not be empty");
        assert!(
            flow_id.0.len() > 30,
            "FlowId should be a UUID (length > 30), got {}",
            flow_id.0.len()
        );

        // Verify the flow is retrievable.
        let state = engine.get_flow_state(&flow_id).await;
        assert!(state.is_some(), "Flow should be retrievable after creation");
        assert_eq!(state.unwrap().status, FlowStatus::Pending);
    }

    #[tokio::test]
    async fn test_simple_3_step_sequential_flow() {
        let engine = new_engine();

        // step1 -> step2 -> step3
        let steps = vec![
            make_step("s1", vec![], ApprovalRequirement::Never),
            make_step("s2", vec!["s1"], ApprovalRequirement::Never),
            make_step("s3", vec!["s2"], ApprovalRequirement::Never),
        ];

        let flow = Flow::new("sequential", steps);
        let flow_id = engine.create_flow(flow).await.unwrap();

        let result = engine.execute_flow(&flow_id).await.unwrap();

        assert_eq!(result.status, FlowStatus::Completed);
        assert_eq!(result.succeeded, 3);
        assert_eq!(result.failed, 0);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.total_steps, 3);
    }

    #[tokio::test]
    async fn test_approval_pause_resume() {
        let engine = new_engine();

        // step1 (no approval) -> step2 (Always approval) -> step3 (no approval)
        let steps = vec![
            make_step("s1", vec![], ApprovalRequirement::Never),
            make_step("s2", vec!["s1"], ApprovalRequirement::Always),
            make_step("s3", vec!["s2"], ApprovalRequirement::Never),
        ];

        let flow = Flow::new("approval_flow", steps);
        let flow_id = engine.create_flow(flow).await.unwrap();

        // Execute -- will stop at step2 (waiting for approval).
        let _ = engine.execute_flow(&flow_id).await.unwrap();

        // Verify intermediate state.
        let state = engine.get_flow_state(&flow_id).await.unwrap();
        assert_eq!(
            state
                .step_states
                .get(&FlowStepId("s1".to_string()))
                .unwrap()
                .status,
            StepStatus::Succeeded
        );
        assert_eq!(
            state
                .step_states
                .get(&FlowStepId("s2".to_string()))
                .unwrap()
                .status,
            StepStatus::WaitingApproval
        );
        assert_eq!(
            state
                .step_states
                .get(&FlowStepId("s3".to_string()))
                .unwrap()
                .status,
            StepStatus::Pending
        );

        // Pause -> approve -> resume.
        engine.pause_flow(&flow_id).await.unwrap();
        engine
            .approve_step(&flow_id, &FlowStepId("s2".to_string()))
            .await
            .unwrap();

        let result = engine.resume_flow(&flow_id).await.unwrap();
        assert_eq!(
            result.status,
            FlowStatus::Completed,
            "Flow should complete after approval + resume"
        );
        assert_eq!(result.succeeded, 3);
    }

    #[tokio::test]
    async fn test_cancel_mid_execution() {
        let engine = new_engine();

        let steps = vec![
            make_step("s1", vec![], ApprovalRequirement::Never),
            make_step("s2", vec!["s1"], ApprovalRequirement::Always),
            make_step("s3", vec!["s2"], ApprovalRequirement::Never),
        ];

        let flow = Flow::new("cancel_flow", steps);
        let flow_id = engine.create_flow(flow).await.unwrap();

        // Execute until approval gate.
        let _ = engine.execute_flow(&flow_id).await.unwrap();

        let state = engine.get_flow_state(&flow_id).await.unwrap();
        assert_eq!(state.status, FlowStatus::Running);
        assert_eq!(
            state
                .step_states
                .get(&FlowStepId("s2".to_string()))
                .unwrap()
                .status,
            StepStatus::WaitingApproval
        );

        // Cancel.
        engine.cancel_flow(&flow_id).await.unwrap();

        let state = engine.get_flow_state(&flow_id).await.unwrap();
        assert_eq!(state.status, FlowStatus::Cancelled);
    }

    #[tokio::test]
    async fn test_topological_sort_complex_dag() {
        let engine = new_engine();

        //          ┌───┐
        //          │s1 │
        //          └─┬─┘
        //         ┌──┴──┐
        //        ┌▼┐   ┌▼┐
        //        │s2│  │s3│
        //        └┬┘   └┬┘
        //         └──┬──┘
        //          ┌─▼─┐
        //          │s4 │
        //          └─┬─┘
        //          ┌─▼─┐
        //          │s5 │
        //          └───┘
        let steps = vec![
            make_step("s1", vec![], ApprovalRequirement::Never),
            make_step("s2", vec!["s1"], ApprovalRequirement::Never),
            make_step("s3", vec!["s1"], ApprovalRequirement::Never),
            make_step("s4", vec!["s2", "s3"], ApprovalRequirement::Never),
            make_step("s5", vec!["s4"], ApprovalRequirement::Never),
        ];

        let flow = Flow::new("complex_dag", steps);
        let flow_id = engine.create_flow(flow).await.unwrap();

        let result = engine.execute_flow(&flow_id).await.unwrap();
        assert_eq!(result.status, FlowStatus::Completed);
        assert_eq!(result.succeeded, 5);
        assert_eq!(result.total_steps, 5);
    }

    #[tokio::test]
    async fn test_topological_sort_detects_cycle() {
        let engine = new_engine();

        // Cycle: s1 -> s2 -> s1
        let steps = vec![
            make_step("s1", vec!["s2"], ApprovalRequirement::Never),
            make_step("s2", vec!["s1"], ApprovalRequirement::Never),
        ];

        let flow = Flow::new("cycle_dag", steps);
        let flow_id = engine.create_flow(flow).await.unwrap();

        let result = engine.execute_flow(&flow_id).await;
        match result {
            Err(FlowError::InvalidDag(msg)) => {
                assert!(
                    msg.contains("Cycle"),
                    "Error message should mention 'Cycle', got: {msg}"
                );
            }
            other => panic!("Expected InvalidDag error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_parallel_dag_execution() {
        let engine = new_engine();

        // s1 has no deps.
        // s2, s3 both depend on s1 only (can run in parallel).
        // s4 depends on s2 and s3.
        let steps = vec![
            make_step("s1", vec![], ApprovalRequirement::Never),
            make_step("s2", vec!["s1"], ApprovalRequirement::Never),
            make_step("s3", vec!["s1"], ApprovalRequirement::Never),
            make_step("s4", vec!["s2", "s3"], ApprovalRequirement::Never),
        ];

        let flow = Flow::new("parallel_dag", steps);
        let flow_id = engine.create_flow(flow).await.unwrap();

        let result = engine.execute_flow(&flow_id).await.unwrap();
        assert_eq!(result.status, FlowStatus::Completed);
        assert_eq!(result.succeeded, 4);
    }

    #[tokio::test]
    async fn test_recover_interrupted_flows() {
        let engine = new_engine();

        // Create a flow with an approval step so execute_flow leaves it Running.
        let steps = vec![
            make_step("s1", vec![], ApprovalRequirement::Never),
            make_step("s2", vec!["s1"], ApprovalRequirement::Always),
        ];

        let flow = Flow::new("recoverable", steps);
        let flow_id = engine.create_flow(flow).await.unwrap();
        let _ = engine.execute_flow(&flow_id).await.unwrap();

        // Flow should be Running (waiting for approval).
        let interrupted = engine.recover_interrupted_flows().await.unwrap();
        assert!(
            interrupted.contains(&flow_id),
            "Flow should be listed as interrupted"
        );
    }

    #[tokio::test]
    async fn test_reject_step() {
        let engine = new_engine();

        let steps = vec![
            make_step("s1", vec![], ApprovalRequirement::Never),
            make_step("s2", vec!["s1"], ApprovalRequirement::Always),
        ];

        let flow = Flow::new("reject_test", steps);
        let flow_id = engine.create_flow(flow).await.unwrap();
        let _ = engine.execute_flow(&flow_id).await.unwrap();

        // Reject step2.
        engine
            .reject_step(&flow_id, &FlowStepId("s2".to_string()))
            .await
            .unwrap();

        let state = engine.get_flow_state(&flow_id).await.unwrap();
        assert_eq!(
            state
                .step_states
                .get(&FlowStepId("s2".to_string()))
                .unwrap()
                .status,
            StepStatus::Rejected
        );
    }

    #[tokio::test]
    async fn test_list_flows() {
        let engine = new_engine();

        let steps_a = vec![make_step("s1", vec![], ApprovalRequirement::Never)];
        let steps_b = vec![make_step("s1", vec![], ApprovalRequirement::Never)];

        let _ = engine.create_flow(Flow::new("flow_a", steps_a)).await.unwrap();
        let _ = engine.create_flow(Flow::new("flow_b", steps_b)).await.unwrap();

        let summaries = engine.list_flows().await.unwrap();
        assert_eq!(summaries.len(), 2);

        let names: Vec<&str> = summaries.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"flow_a"));
        assert!(names.contains(&"flow_b"));
    }
}
