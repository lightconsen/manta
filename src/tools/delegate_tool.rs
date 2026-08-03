//! Subagent Delegation Tool
//!
//! This tool allows an agent to spawn child agents for parallel task execution.
//! Implements depth limiting, budget sharing, and tool restrictions for
//! children.
//!
//! Integrates with [`SubagentRegistry`] for lifecycle tracking and metrics, and
//! supports opt-in [`ToolHooks`] for audit/observability.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::{Tool, ToolContext, ToolExecutionResult};
use crate::agent::budget::IterationBudget;
use crate::agent::subagent_registry::SubagentRegistry;
use crate::delegation::{
    DelegationConfig, DelegationCoordinator, DelegationEvent, DelegationScope, DelegationTaskStore,
    NewTask,
};
use crate::tools::hooks::ToolHooks;
use crate::tools::sdk::ToolCapabilities;
use uuid::Uuid;

/// Tools stripped from a child's requested allowlist.
///
/// `delegate` is listed for documentation and the spawn-time warning only —
/// its real enforcement is depth-based inside
/// [`DelegationScope::is_tool_allowed`], so interior nodes keep recursion
/// while leaves lose it. The remaining tools match
/// [`DELEGATION_BLOCKED_TOOLS`](crate::delegation::scope::DELEGATION_BLOCKED_TOOLS).
const BLOCKED_TOOLS: &[&str] = &[
    "delegate",
    "clarify",
    "memory",
    "send_message",
    "execute_code",
];

/// Task specification for child agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    /// Task description/prompt
    pub prompt: String,
    /// Expected output format
    pub output_format: Option<String>,
    /// Maximum iterations for child
    pub max_iterations: Option<usize>,
    /// Tools allowed for child (empty = all except blocked)
    pub allowed_tools: Vec<String>,
    /// Context to pass to child
    pub context: HashMap<String, serde_json::Value>,
    /// Target agent type for routing (e.g., "coder", "reviewer"). Defaults to
    /// "delegate".
    #[serde(default)]
    pub target_agent: Option<String>,
    /// Optional shared-task id for the child (registry run id).  When set, the
    /// child's shared state is tracked under this task.
    #[serde(default)]
    pub task_id: Option<String>,
}

/// Child agent handle
#[derive(Debug, Clone)]
pub struct ChildAgent {
    /// Unique ID
    pub id: String,
    /// Parent agent ID
    pub parent_id: String,
    /// Task specification
    pub task: TaskSpec,
    /// Current status
    pub status: ChildStatus,
    /// Creation time
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Result (if completed)
    pub result: Option<String>,
    /// Error (if failed)
    pub error: Option<String>,
    /// Shared budget reference
    pub budget: IterationBudget,
    /// Current iteration count
    pub iterations: Arc<AtomicUsize>,
}

/// Child agent status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildStatus {
    /// Waiting to start
    Pending,
    /// Currently running
    Running,
    /// Completed successfully
    Completed,
    /// Failed with error
    Failed,
    /// Cancelled by parent
    Cancelled,
}

/// Delegation tracker for managing child agents
#[derive(Debug, Default)]
pub struct DelegationTracker {
    /// Active child agents
    children: Arc<RwLock<HashMap<String, ChildAgent>>>,
    /// Current delegation depth of the agent this tracker belongs to
    depth: usize,
    /// Maximum allowed children
    max_children: usize,
    /// Maximum nesting depth (agents at or beyond this may not delegate)
    max_depth: usize,
}

impl DelegationTracker {
    /// Create a new delegation tracker with default limits.
    pub fn new(depth: usize) -> Self {
        Self::with_limits(depth, DelegationConfig::default())
    }

    /// Create a new delegation tracker with explicit limits.
    pub fn with_limits(depth: usize, config: DelegationConfig) -> Self {
        Self {
            children: Arc::new(RwLock::new(HashMap::new())),
            depth,
            max_children: config.max_children,
            max_depth: config.max_depth as usize,
        }
    }

    /// Replace the depth/concurrency limits (e.g. from a config reload).
    pub fn set_limits(&mut self, config: DelegationConfig) {
        self.max_children = config.max_children;
        self.max_depth = config.max_depth as usize;
    }

    /// Check if delegation is allowed
    pub async fn can_delegate(&self) -> bool {
        if self.depth >= self.max_depth {
            return false;
        }
        let children = self.children.read().await;
        children.len() < self.max_children
    }

    /// Get current child count
    pub async fn child_count(&self) -> usize {
        let children = self.children.read().await;
        children.len()
    }

    /// Register a new child agent
    pub async fn register_child(&self, child: ChildAgent) {
        let mut children = self.children.write().await;
        children.insert(child.id.clone(), child);
    }

    /// Get a child agent by ID
    pub async fn get_child(&self, id: &str) -> Option<ChildAgent> {
        let children = self.children.read().await;
        children.get(id).cloned()
    }

    /// Update child status
    pub async fn update_status(&self, id: &str, status: ChildStatus) {
        let mut children = self.children.write().await;
        if let Some(child) = children.get_mut(id) {
            child.status = status;
        }
    }

    /// Set child result
    pub async fn set_result(&self, id: &str, result: String) {
        let mut children = self.children.write().await;
        if let Some(child) = children.get_mut(id) {
            child.status = ChildStatus::Completed;
            child.result = Some(result);
        }
    }

    /// Set child error
    pub async fn set_error(&self, id: &str, error: String) {
        let mut children = self.children.write().await;
        if let Some(child) = children.get_mut(id) {
            child.status = ChildStatus::Failed;
            child.error = Some(error);
        }
    }

    /// List all children
    pub async fn list_children(&self) -> Vec<ChildAgent> {
        let children = self.children.read().await;
        children.values().cloned().collect()
    }

    /// Remove a child
    pub async fn remove_child(&self, id: &str) -> Option<ChildAgent> {
        let mut children = self.children.write().await;
        children.remove(id)
    }
}

/// Trait for looking up running agents by name/type.
///
/// Used by [`DelegateTool`] to route child tasks to specific agents
/// based on the `target_agent` field in [`TaskSpec`].
#[async_trait]
pub trait AgentResolver: Send + Sync {
    /// Resolve a running agent by name (e.g. "coder", "reviewer").
    /// Returns `None` if no agent with that name is available.
    async fn resolve(&self, name: &str) -> Option<Arc<crate::agent::Agent>>;
}

/// Delegate tool for spawning child agents
pub struct DelegateTool {
    tracker: DelegationTracker,
    /// Optional agent for executing child tasks
    agent: Option<Arc<crate::agent::Agent>>,
    /// Shared registry for cross-cutting subagent lifecycle tracking
    registry: Arc<SubagentRegistry>,
    /// Optional hooks for pre/post execution observability
    hooks: ToolHooks,
    /// Optional resolver for `target_agent` routing — when set, children
    /// are routed to the named agent instead of `self.agent`.
    agent_resolver: Option<Arc<dyn AgentResolver>>,
    /// Optional shared task state store.  When set, every spawned child gets a
    /// `delegation_tasks` row and can read/write shared state via `task_state`.
    store: Option<Arc<DelegationTaskStore>>,
    /// Optional handoff coordinator.  When set, a child that finishes while a
    /// sibling/descendant is `waiting_handoff` triggers successor continuation.
    coordinator: Option<Arc<DelegationCoordinator>>,
}

impl std::fmt::Debug for DelegateTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegateTool")
            .field("tracker", &self.tracker)
            .field("has_agent", &self.agent.is_some())
            .field("hooks", &self.hooks)
            .finish()
    }
}

impl DelegateTool {
    /// Create a new delegate tool with default limits.
    pub fn new(depth: usize) -> Self {
        Self::new_with_config(depth, DelegationConfig::default())
    }

    /// Create a new delegate tool with an agent for execution.
    pub fn with_agent(depth: usize, agent: Arc<crate::agent::Agent>) -> Self {
        Self::with_agent_and_config(depth, agent, DelegationConfig::default())
    }

    /// Create a new delegate tool with explicit depth/concurrency limits.
    fn new_with_config(depth: usize, config: DelegationConfig) -> Self {
        Self {
            tracker: DelegationTracker::with_limits(depth, config.clone()),
            agent: None,
            registry: Arc::new(SubagentRegistry::new(config.max_depth, config.max_children)),
            hooks: ToolHooks::new(),
            agent_resolver: None,
            store: None,
            coordinator: None,
        }
    }

    /// Create a new delegate tool with an agent and explicit limits.
    fn with_agent_and_config(
        depth: usize,
        agent: Arc<crate::agent::Agent>,
        config: DelegationConfig,
    ) -> Self {
        Self {
            tracker: DelegationTracker::with_limits(depth, config.clone()),
            agent: Some(agent),
            registry: Arc::new(SubagentRegistry::new(config.max_depth, config.max_children)),
            hooks: ToolHooks::new(),
            agent_resolver: None,
            store: None,
            coordinator: None,
        }
    }

    /// Create root-level delegate tool (depth 0)
    pub fn root() -> Self {
        Self::new(0)
    }

    /// Attach a shared [`SubagentRegistry`] (e.g. from a higher-level
    /// supervisor).
    pub fn with_registry(mut self, registry: Arc<SubagentRegistry>) -> Self {
        self.registry = registry;
        self
    }

    /// Attach execution hooks.
    pub fn with_hooks(mut self, hooks: ToolHooks) -> Self {
        self.hooks = hooks;
        self
    }

    /// Attach an [`AgentResolver`] for `target_agent` routing.
    ///
    /// When set, the `target_agent` field in [`TaskSpec`] is used to look
    /// up the appropriate agent. Falls back to `self.agent` when the target
    /// is not found or not specified.
    pub fn with_agent_resolver(mut self, resolver: Arc<dyn AgentResolver>) -> Self {
        self.agent_resolver = Some(resolver);
        self
    }

    /// Attach a shared delegation task store.  When set, every spawned child
    /// gets a shared-state row that sibling/descendant agents can read and
    /// write through the `task_state` tool.
    pub fn with_task_store(mut self, store: Arc<DelegationTaskStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Attach a handoff coordinator.  When set, a child that finishes while a
    /// task under the same root is `waiting_handoff` drives successor
    /// continuation via the coordinator.
    pub fn with_coordinator(mut self, coordinator: Arc<DelegationCoordinator>) -> Self {
        self.coordinator = Some(coordinator);
        self
    }

    /// Override the delegation depth/concurrency limits.
    ///
    /// Rebuilds the shared [`SubagentRegistry`] and tracker so both enforce
    /// the same bounds (defaults: depth 3, 3 concurrent children).
    pub fn with_delegation_config(mut self, config: DelegationConfig) -> Self {
        self.tracker.set_limits(config.clone());
        self.registry = Arc::new(SubagentRegistry::new(config.max_depth, config.max_children));
        self
    }

    /// Access the underlying registry for metrics / status queries.
    pub fn registry(&self) -> &Arc<SubagentRegistry> {
        &self.registry
    }

    /// Spawn a child agent
    ///
    /// `parent_scope` is the caller's delegation scope (`None` for a
    /// top-level delegation).  The child's own scope is derived from it: one
    /// level deeper, sharing the same tree root.
    async fn spawn_child(
        &self,
        task: TaskSpec,
        parent_budget: Option<IterationBudget>,
        parent_id: String,
        parent_scope: Option<DelegationScope>,
    ) -> crate::Result<ChildAgent> {
        let budget = parent_budget.unwrap_or_else(|| IterationBudget::new(50));
        let iterations = Arc::new(AtomicUsize::new(0));

        // Compute the child's intended depth before asking the registry, which
        // validates it against its configured max depth.
        let depth = match &parent_scope {
            Some(ps) => ps.depth + 1,
            None => 1,
        };
        let max_depth = self.registry.max_depth();
        let root_id = match &parent_scope {
            Some(ps) => ps.root_id.clone(),
            None => Uuid::new_v4().to_string(),
        };
        let parent_task_id = parent_scope.as_ref().map(|ps| ps.task_id.clone());

        // Determine which agent to use for child execution:
        // 1. If target_agent is set and we have a resolver, look it up
        // 2. Fall back to self.agent if not found or no target specified
        let child_agent = if let Some(ref resolver) = self.agent_resolver {
            if let Some(target) = &task.target_agent {
                resolver
                    .resolve(target)
                    .await
                    .or_else(|| self.agent.clone())
            } else {
                self.agent.clone()
            }
        } else {
            self.agent.clone()
        };

        let agent_type = task
            .target_agent
            .clone()
            .unwrap_or_else(|| "delegate".to_string());

        // Build the execution closure. The registry will supply the run_id, which
        // becomes the child id so local tracking and registry tracking share a
        // key; the child's delegation scope is built inside the closure once
        // that run_id is known.
        let registry = Arc::clone(&self.registry);
        let store_opt = self.store.clone();
        let reg_task = task.clone();
        // The child's execution tracker carries its actual depth so any
        // depth-based gating (e.g. `can_delegate`) reflects the child's level
        // in the tree rather than the shared tool's construction depth (0).
        let mut reg_tracker = self.tracker.clone();
        reg_tracker.depth = depth as usize;
        let iterations_bg = iterations.clone();
        let coordinator = self.coordinator.clone();
        let root_id_bg = root_id.clone();
        let parent_task_id_bg = parent_task_id.clone();
        let agent_id_owned = agent_type.clone();
        let task_fn = move |run_id: String, _task_str: String| {
            let reg_task = reg_task.clone();
            let reg_tracker = reg_tracker.clone();
            let iterations_bg = iterations_bg.clone();
            let child_agent = child_agent.clone();
            let registry = Arc::clone(&registry);
            let store_opt = store_opt.clone();
            let coordinator = coordinator.clone();
            let agent_id = agent_id_owned.clone();
            let scope = DelegationScope {
                root_id: root_id_bg.clone(),
                task_id: run_id.clone(),
                parent_task_id: parent_task_id_bg.clone(),
                depth,
                max_depth,
                allowed_tools: if reg_task.allowed_tools.is_empty() {
                    None
                } else {
                    Some(reg_task.allowed_tools.clone())
                },
                max_iterations: reg_task.max_iterations,
            };
            async move {
                execute_child_task(
                    run_id,
                    reg_task,
                    reg_tracker,
                    iterations_bg,
                    child_agent,
                    registry,
                    store_opt,
                    scope,
                    agent_id,
                    coordinator,
                )
                .await;
            }
        };

        // Ask the registry to enforce depth/concurrency limits and assign a run id.
        // If the registry rejects the spawn, no child is registered locally.
        let run_id = self
            .registry
            .spawn(&parent_id, &agent_type, &task.prompt, depth, task_fn)
            .await?;

        let child = ChildAgent {
            id: run_id,
            parent_id: parent_id.clone(),
            task: task.clone(),
            status: ChildStatus::Pending,
            created_at: chrono::Utc::now(),
            result: None,
            error: None,
            budget: budget.child(),
            iterations: iterations.clone(),
        };

        // Register the child with the local tracker
        self.tracker.register_child(child.clone()).await;

        info!(
            "Spawned child agent {} for task: {}",
            child.id,
            task.prompt.chars().take(50).collect::<String>()
        );

        Ok(child)
    }
}

/// Execute a child task using the provided agent, reporting outcomes to both
/// the local [`DelegationTracker`] and the shared [`SubagentRegistry`].
///
/// When a task store is attached, the child gets a `delegation_tasks` row and
/// the caller-supplied [`DelegationScope`] is threaded through its message
/// metadata so it can read and write shared state via the `task_state` tool.
///
/// `coordinator`, when present, advances pending handoffs under the child's
/// tree root after the child finishes.
#[allow(clippy::too_many_arguments)] // closure-captured execution context for a spawned child
pub(crate) fn execute_child_task(
    child_id: String,
    task: TaskSpec,
    tracker: DelegationTracker,
    iterations: Arc<AtomicUsize>,
    agent: Option<Arc<crate::agent::Agent>>,
    registry: Arc<SubagentRegistry>,
    store: Option<Arc<DelegationTaskStore>>,
    scope: DelegationScope,
    agent_id: String,
    coordinator: Option<Arc<DelegationCoordinator>>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    // Boxed (type-erased) future: `execute_child_task -> maybe_advance ->
    // successor spawn -> execute_child_task` is a genuinely cyclic call graph,
    // so a concrete `impl Future` would recurse in the type system (E0391).
    Box::pin(async move {
        tracker.update_status(&child_id, ChildStatus::Running).await;

        debug!("Child {} starting execution", child_id);

        // Row linkage: the tree root and the child's parent row come from the scope.
        let root_id = scope.root_id.clone();
        let depth = scope.depth;
        let parent_id = scope.parent_task_id.clone();

        // Create the shared-state row and record a start event.
        if let Some(store) = &store {
            let title: String = task.prompt.chars().take(120).collect();
            if let Err(e) = store
                .create_task(NewTask {
                    id: &child_id,
                    root_id: &root_id,
                    parent_id: parent_id.as_deref(),
                    depth,
                    agent_id: &agent_id,
                    title: &title,
                })
                .await
            {
                warn!("Failed to create delegation task '{}': {}", child_id, e);
            }
            if let Err(e) = store
                .append_event(
                    &child_id,
                    &DelegationEvent::new(
                        &agent_id,
                        "started",
                        task.prompt.chars().take(80).collect::<String>(),
                    ),
                )
                .await
            {
                warn!("Failed to record start event for '{}': {}", child_id, e);
            }
        }

        if let Some(agent) = agent {
            // Create incoming message for the child task
            let message = crate::channels::IncomingMessage::new(
                format!("child:{}", child_id),
                format!("delegation:{}", child_id),
                &task.prompt,
            )
            .with_metadata(
                crate::channels::MessageMetadata::new()
                    .with_extra("child_id", child_id.clone())
                    .with_extra("output_format", task.output_format.clone().unwrap_or_default())
                    .with_extra("allowed_tools", task.allowed_tools.join(","))
                    .with_extra(
                        crate::delegation::DELEGATION_SCOPE_KEY,
                        serde_json::to_value(&scope).unwrap_or(serde_json::Value::Null),
                    ),
            );

            // Build a debug-logging progress callback so child tool activity
            // surfaces in logs even though there is no parent callback to forward to.
            let child_id_cb = child_id.clone();
            let progress_cb: crate::agent::ProgressCallback = Arc::new(move |event| {
                let cid = child_id_cb.clone();
                Box::pin(async move {
                    match event {
                        crate::agent::ProgressEvent::ToolCalling { name, arguments } => {
                            debug!("Child {} calling tool {}: {}", cid, name, arguments);
                        }
                        crate::agent::ProgressEvent::ToolResult { name, result, .. } => {
                            debug!("Child {} tool {} result: {} chars", cid, name, result.len());
                        }
                        crate::agent::ProgressEvent::Error { message } => {
                            warn!("Child {} progress error: {}", cid, message);
                        }
                        _ => {}
                    }
                })
            });

            // Process the task through the agent with progress visibility
            match agent
                .process_message_with_progress(message, progress_cb)
                .await
            {
                Ok(response) => {
                    iterations.fetch_add(1, Ordering::SeqCst);

                    info!(
                        "Child {} completed successfully. Response: {} chars",
                        child_id,
                        response.content.len()
                    );

                    // Format result based on output_format if specified
                    let result = if let Some(format) = &task.output_format {
                        format!("Output format ({}): {}", format, response.content)
                    } else {
                        response.content.clone()
                    };

                    tracker.set_result(&child_id, result.clone()).await;
                    registry.complete_run(&child_id, Ok(result)).await;

                    // Write the outcome back to the shared task record.
                    if let Some(store) = &store {
                        // A task that requested a handoff must keep `waiting_handoff`
                        // so the coordinator can advance it.
                        preserve_handoff_and_set_status(store, &child_id, "completed").await;
                        if let Err(e) = store
                            .append_event(
                                &child_id,
                                &DelegationEvent::new(
                                    &agent_id,
                                    "completed",
                                    format!("output: {} chars", response.content.len()),
                                ),
                            )
                            .await
                        {
                            warn!("Failed to record completion event for '{}': {}", child_id, e);
                        }
                    }
                }
                Err(e) => {
                    error!("Child {} failed: {}", child_id, e);
                    let err_msg = format!("Task execution failed: {}", e);
                    tracker.set_error(&child_id, err_msg.clone()).await;
                    registry.complete_run(&child_id, Err(err_msg)).await;

                    if let Some(store) = &store {
                        preserve_handoff_and_set_status(store, &child_id, "failed").await;
                    }
                }
            }
        } else {
            // No agent configured - log warning and mark as failed
            warn!(
                "No agent configured for child {}. Task would execute with prompt: {}",
                child_id, task.prompt
            );
            let err_msg = "No agent configured for delegation".to_string();
            tracker.set_error(&child_id, err_msg.clone()).await;
            registry.complete_run(&child_id, Err(err_msg)).await;

            if let Some(store) = &store {
                preserve_handoff_and_set_status(store, &child_id, "failed").await;
            }
        }

        // Drive any pending handoffs in this tree (successor continuation).  Runs
        // on both the success and failure paths so a handoff requested just before
        // a child finished is still picked up.
        if let Some(coordinator) = &coordinator {
            if let Err(e) = coordinator.maybe_advance(&root_id).await {
                warn!("Failed to advance delegation tree '{}': {}", root_id, e);
            }
        }

        debug!("Child {} execution completed", child_id);
    })
}

/// Set a delegation task's status unless it is already `waiting_handoff`.
///
/// A child that requested a handoff must keep `waiting_handoff` so the
/// coordinator can pick it up and spawn the successor; overwriting it with
/// `completed`/`failed` would silently drop the handoff.
async fn preserve_handoff_and_set_status(
    store: &DelegationTaskStore,
    child_id: &str,
    status: &str,
) {
    let pending_handoff = store
        .get_task(child_id)
        .await
        .ok()
        .flatten()
        .is_some_and(|task| task.is_waiting_handoff());
    if pending_handoff {
        return;
    }
    if let Err(e) = store.set_status(child_id, status).await {
        warn!("Failed to mark delegation task '{}' {}: {}", child_id, status, e);
    }
}

#[async_trait]
impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate"
    }

    fn description(&self) -> &str {
        r#"Spawn a child agent to handle a subtask in parallel.

Use this tool to:
- Break complex tasks into parallel subtasks
- Delegate work to specialized agents
- Process multiple items concurrently

Limitations:
- Maximum 3 concurrent children per parent
- Delegation nests up to 3 levels deep; agents at the deepest level cannot
  delegate further
- Child agents cannot use: clarify, memory, send_message, execute_code
- Children share parent's iteration budget

The child agent will execute the task independently and return results.
Progress and results are relayed to the parent."#
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["spawn", "status", "list", "cancel", "metrics"],
                    "description": "Action to perform"
                },
                "task": {
                    "type": "object",
                    "description": "Task specification (for spawn)",
                    "properties": {
                        "prompt": {
                            "type": "string",
                            "description": "Task description/prompt for child"
                        },
                        "output_format": {
                            "type": "string",
                            "description": "Expected output format"
                        },
                        "max_iterations": {
                            "type": "integer",
                            "description": "Maximum iterations for child"
                        },
                        "allowed_tools": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Tools allowed for child (empty = all except blocked)"
                        }
                    },
                    "required": ["prompt"]
                },
                "child_id": {
                    "type": "string",
                    "description": "Child agent ID (for status/cancel)"
                }
            },
            "required": ["action"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: true,
            risk_level: crate::tools::approval::RiskLevel::High,
            categories: vec!["system".to_string(), "delegate".to_string()],
            ..Default::default()
        }
    }

    fn is_available(&self, context: &ToolContext) -> bool {
        !context.sandboxed() || !context.allowed_commands().is_empty()
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        // ── before hooks ─────────────────────────────────────────────────
        self.hooks.run_before(self.name(), &args).await;

        let result = self.execute_inner(args.clone(), context).await;

        // ── after hooks ──────────────────────────────────────────────────
        let exec_result = match &result {
            Ok(r) => r.clone(),
            Err(e) => ToolExecutionResult::error(e.to_string()),
        };
        self.hooks.run_after(self.name(), &args, &exec_result).await;

        result
    }
}

impl DelegateTool {
    async fn execute_inner(
        &self,
        args: serde_json::Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let action = args["action"].as_str().ok_or_else(|| {
            crate::error::SyscityError::Validation("action is required".to_string())
        })?;

        match action {
            "spawn" => {
                // Fast pre-check using the shared registry, which is the authority
                // for depth and concurrency limits.
                let current_count = self.registry.active_count().await;
                let max_children = self.registry.max_concurrent();
                if current_count >= max_children {
                    return Ok(ToolExecutionResult::error(format!(
                        "Maximum children ({}) already active. Cannot spawn more.",
                        max_children
                    )));
                }

                let task_json = &args["task"];
                let prompt = task_json["prompt"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation("task.prompt is required".to_string())
                })?;

                // Parse requested tools, then strip BLOCKED_TOOLS. `delegate`
                // is only stripped when the child cannot recurse (a leaf) —
                // interior nodes keep it so they can delegate further. The
                // scope re-enforces both rules at execution time.
                let requested_tools: Vec<String> = task_json["allowed_tools"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                let child_depth = match &context.delegation {
                    Some(ps) => ps.depth + 1,
                    None => 1,
                };
                let can_child_delegate = child_depth < self.registry.max_depth();

                let blocked_requested: Vec<&str> = requested_tools
                    .iter()
                    .filter(|t| {
                        BLOCKED_TOOLS.contains(&t.as_str())
                            && !(*t == "delegate" && can_child_delegate)
                    })
                    .map(|t| t.as_str())
                    .collect();

                if !blocked_requested.is_empty() {
                    warn!(
                        "Child agent spawn: removing {} blocked tool(s) from allowed list: {:?}",
                        blocked_requested.len(),
                        blocked_requested
                    );
                }

                let allowed_tools: Vec<String> = requested_tools
                    .into_iter()
                    .filter(|t| {
                        !BLOCKED_TOOLS.contains(&t.as_str())
                            || (*t == "delegate" && can_child_delegate)
                    })
                    .collect();

                let task = TaskSpec {
                    prompt: prompt.to_string(),
                    output_format: task_json["output_format"].as_str().map(String::from),
                    max_iterations: task_json["max_iterations"].as_u64().map(|v| v as usize),
                    allowed_tools,
                    context: HashMap::new(),
                    target_agent: task_json["target_agent"].as_str().map(String::from),
                    task_id: task_json["task_id"].as_str().map(String::from),
                };

                let child = self
                    .spawn_child(
                        task,
                        None,
                        context.conversation_id.clone(),
                        context.delegation.clone(),
                    )
                    .await?;

                let depth = match &context.delegation {
                    Some(scope) => scope.depth + 1,
                    None => 1,
                };

                Ok(ToolExecutionResult::success(format!("Spawned child agent: {}", child.id))
                    .with_data(json!({
                        "child_id": child.id,
                        "status": child.status,
                        "depth": depth,
                        "max_depth": self.registry.max_depth(),
                    })))
            }

            "status" => {
                let child_id = args["child_id"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "child_id is required for status".to_string(),
                    )
                })?;

                match self.tracker.get_child(child_id).await {
                    Some(child) => Ok(ToolExecutionResult::success(format!(
                        "Child {} status: {:?}",
                        child_id, child.status
                    ))
                    .with_data(json!({
                        "child_id": child.id,
                        "status": child.status,
                        "result": child.result,
                        "error": child.error,
                        "created_at": child.created_at.to_rfc3339(),
                    }))),
                    None => Ok(ToolExecutionResult::error(format!("Child {} not found", child_id))),
                }
            }

            "list" => {
                let children = self.tracker.list_children().await;
                let summary: Vec<serde_json::Value> = children.iter().map(|c| {
                    json!({
                        "id": c.id,
                        "status": c.status,
                        "prompt_preview": c.task.prompt.chars().take(50).collect::<String>() + "...",
                    })
                }).collect();

                Ok(ToolExecutionResult::success(format!("{} active children", children.len()))
                    .with_data(json!({
                        "children": summary,
                        "count": children.len(),
                        "max_children": self.registry.max_concurrent(),
                    })))
            }

            "cancel" => {
                let child_id = args["child_id"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "child_id is required for cancel".to_string(),
                    )
                })?;

                if let Some(_child) = self.tracker.remove_child(child_id).await {
                    // Also kill in the registry so metrics stay correct.
                    if let Err(e) = self.registry.kill(child_id).await {
                        warn!("Failed to kill child agent '{}': {}", child_id, e);
                    }
                    info!("Cancelled child agent: {}", child_id);
                    Ok(ToolExecutionResult::success(format!("Cancelled child {}", child_id)))
                } else {
                    Ok(ToolExecutionResult::error(format!("Child {} not found", child_id)))
                }
            }

            "metrics" => {
                let m = self.registry.metrics().await;
                Ok(ToolExecutionResult::success(format!(
                    "Subagent metrics: {} spawned, {} completed, {} failed, {} killed",
                    m.total_spawned, m.total_completed, m.total_failed, m.total_killed
                ))
                .with_data(json!({
                    "total_spawned": m.total_spawned,
                    "total_completed": m.total_completed,
                    "total_failed": m.total_failed,
                    "total_killed": m.total_killed,
                    "active_count": self.registry.active_count().await,
                })))
            }

            _ => Err(crate::error::SyscityError::Validation(format!("Unknown action: {}", action))),
        }
    }
}

impl Clone for DelegationTracker {
    fn clone(&self) -> Self {
        Self {
            children: Arc::clone(&self.children),
            depth: self.depth,
            max_children: self.max_children,
            max_depth: self.max_depth,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delegation_tracker() {
        let tracker = DelegationTracker::new(0);
        assert_eq!(tracker.depth, 0);
    }

    #[test]
    fn test_task_spec_creation() {
        let task = TaskSpec {
            prompt: "Test task".to_string(),
            output_format: Some("json".to_string()),
            max_iterations: Some(10),
            allowed_tools: vec!["file_read".to_string()],
            context: HashMap::new(),
            target_agent: None,
            task_id: None,
        };
        assert_eq!(task.prompt, "Test task");
    }

    #[test]
    fn test_child_status_serialization() {
        let status = ChildStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");
    }

    #[tokio::test]
    async fn test_delegation_tracker_can_delegate_at_depth_0() {
        let tracker = DelegationTracker::new(0);
        assert!(tracker.can_delegate().await);
    }

    #[tokio::test]
    async fn test_delegation_tracker_can_delegate_at_max_depth() {
        let cfg = DelegationConfig::default();
        let tracker = DelegationTracker::with_limits(cfg.max_depth as usize, cfg.clone());
        assert!(!tracker.can_delegate().await);

        let tracker = DelegationTracker::with_limits(cfg.max_depth as usize + 1, cfg);
        assert!(!tracker.can_delegate().await);
    }

    #[tokio::test]
    async fn test_delegation_tracker_can_delegate_at_max_children() {
        let cfg = DelegationConfig::default();
        let tracker = DelegationTracker::with_limits(0, cfg.clone());
        for i in 0..cfg.max_children {
            let child = ChildAgent {
                id: format!("child-{}", i),
                parent_id: "parent".to_string(),
                task: TaskSpec {
                    prompt: "test".to_string(),
                    output_format: None,
                    max_iterations: None,
                    allowed_tools: vec![],
                    context: HashMap::new(),
                    target_agent: None,
                    task_id: None,
                },
                status: ChildStatus::Pending,
                created_at: chrono::Utc::now(),
                result: None,
                error: None,
                budget: IterationBudget::new(10),
                iterations: Arc::new(AtomicUsize::new(0)),
            };
            tracker.register_child(child).await;
        }
        assert_eq!(tracker.child_count().await, cfg.max_children);
        assert!(!tracker.can_delegate().await);
    }

    #[tokio::test]
    async fn test_delegation_tracker_register_and_get_child() {
        let tracker = DelegationTracker::new(0);
        let child = ChildAgent {
            id: "c1".to_string(),
            parent_id: "p1".to_string(),
            task: TaskSpec {
                prompt: "hello".to_string(),
                output_format: None,
                max_iterations: None,
                allowed_tools: vec![],
                context: HashMap::new(),
                target_agent: None,
                task_id: None,
            },
            status: ChildStatus::Pending,
            created_at: chrono::Utc::now(),
            result: None,
            error: None,
            budget: IterationBudget::new(10),
            iterations: Arc::new(AtomicUsize::new(0)),
        };
        tracker.register_child(child.clone()).await;

        let fetched = tracker.get_child("c1").await;
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().id, "c1");

        assert!(tracker.get_child("missing").await.is_none());
    }

    #[tokio::test]
    async fn test_delegation_tracker_update_status() {
        let tracker = DelegationTracker::new(0);
        let child = ChildAgent {
            id: "c1".to_string(),
            parent_id: "p1".to_string(),
            task: TaskSpec {
                prompt: "test".to_string(),
                output_format: None,
                max_iterations: None,
                allowed_tools: vec![],
                context: HashMap::new(),
                target_agent: None,
                task_id: None,
            },
            status: ChildStatus::Pending,
            created_at: chrono::Utc::now(),
            result: None,
            error: None,
            budget: IterationBudget::new(10),
            iterations: Arc::new(AtomicUsize::new(0)),
        };
        tracker.register_child(child).await;

        tracker.update_status("c1", ChildStatus::Running).await;
        let fetched = tracker.get_child("c1").await.unwrap();
        assert_eq!(fetched.status, ChildStatus::Running);
    }

    #[tokio::test]
    async fn test_delegation_tracker_set_result() {
        let tracker = DelegationTracker::new(0);
        let child = ChildAgent {
            id: "c1".to_string(),
            parent_id: "p1".to_string(),
            task: TaskSpec {
                prompt: "test".to_string(),
                output_format: None,
                max_iterations: None,
                allowed_tools: vec![],
                context: HashMap::new(),
                target_agent: None,
                task_id: None,
            },
            status: ChildStatus::Running,
            created_at: chrono::Utc::now(),
            result: None,
            error: None,
            budget: IterationBudget::new(10),
            iterations: Arc::new(AtomicUsize::new(0)),
        };
        tracker.register_child(child).await;

        tracker.set_result("c1", "done".to_string()).await;
        let fetched = tracker.get_child("c1").await.unwrap();
        assert_eq!(fetched.status, ChildStatus::Completed);
        assert_eq!(fetched.result, Some("done".to_string()));
    }

    #[tokio::test]
    async fn test_delegation_tracker_set_error() {
        let tracker = DelegationTracker::new(0);
        let child = ChildAgent {
            id: "c1".to_string(),
            parent_id: "p1".to_string(),
            task: TaskSpec {
                prompt: "test".to_string(),
                output_format: None,
                max_iterations: None,
                allowed_tools: vec![],
                context: HashMap::new(),
                target_agent: None,
                task_id: None,
            },
            status: ChildStatus::Running,
            created_at: chrono::Utc::now(),
            result: None,
            error: None,
            budget: IterationBudget::new(10),
            iterations: Arc::new(AtomicUsize::new(0)),
        };
        tracker.register_child(child).await;

        tracker.set_error("c1", "oops".to_string()).await;
        let fetched = tracker.get_child("c1").await.unwrap();
        assert_eq!(fetched.status, ChildStatus::Failed);
        assert_eq!(fetched.error, Some("oops".to_string()));
    }

    #[tokio::test]
    async fn test_delegation_tracker_list_children() {
        let tracker = DelegationTracker::new(0);
        for i in 0..3 {
            let child = ChildAgent {
                id: format!("c{}", i),
                parent_id: "p".to_string(),
                task: TaskSpec {
                    prompt: format!("task {}", i),
                    output_format: None,
                    max_iterations: None,
                    allowed_tools: vec![],
                    context: HashMap::new(),
                    target_agent: None,
                    task_id: None,
                },
                status: ChildStatus::Pending,
                created_at: chrono::Utc::now(),
                result: None,
                error: None,
                budget: IterationBudget::new(10),
                iterations: Arc::new(AtomicUsize::new(0)),
            };
            tracker.register_child(child).await;
        }
        let list = tracker.list_children().await;
        assert_eq!(list.len(), 3);
    }

    #[tokio::test]
    async fn test_delegation_tracker_remove_child() {
        let tracker = DelegationTracker::new(0);
        let child = ChildAgent {
            id: "c1".to_string(),
            parent_id: "p1".to_string(),
            task: TaskSpec {
                prompt: "test".to_string(),
                output_format: None,
                max_iterations: None,
                allowed_tools: vec![],
                context: HashMap::new(),
                target_agent: None,
                task_id: None,
            },
            status: ChildStatus::Pending,
            created_at: chrono::Utc::now(),
            result: None,
            error: None,
            budget: IterationBudget::new(10),
            iterations: Arc::new(AtomicUsize::new(0)),
        };
        tracker.register_child(child).await;

        let removed = tracker.remove_child("c1").await;
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, "c1");
        assert!(tracker.get_child("c1").await.is_none());
        assert!(tracker.remove_child("c1").await.is_none());
    }

    #[test]
    fn test_delegate_tool_new() {
        let tool = DelegateTool::new(1);
        let debug = format!("{:?}", tool);
        assert!(debug.contains("DelegateTool"));
        assert!(debug.contains("has_agent: false"));
    }

    #[test]
    fn test_delegate_tool_root() {
        let tool = DelegateTool::root();
        assert_eq!(tool.tracker.depth, 0);
    }

    #[test]
    fn test_delegate_tool_registry_access() {
        let tool = DelegateTool::new(0);
        let _registry = tool.registry();
        // Registry is accessible and non-null
    }

    #[test]
    fn test_child_status_variants() {
        assert_eq!(ChildStatus::Pending, ChildStatus::Pending);
        assert_eq!(ChildStatus::Running, ChildStatus::Running);
        assert_eq!(ChildStatus::Completed, ChildStatus::Completed);
        assert_eq!(ChildStatus::Failed, ChildStatus::Failed);
        assert_eq!(ChildStatus::Cancelled, ChildStatus::Cancelled);
        assert_ne!(ChildStatus::Pending, ChildStatus::Running);
    }

    #[test]
    fn test_blocked_tools_const() {
        assert!(BLOCKED_TOOLS.contains(&"delegate"));
        assert!(BLOCKED_TOOLS.contains(&"clarify"));
        assert!(BLOCKED_TOOLS.contains(&"memory"));
        assert!(BLOCKED_TOOLS.contains(&"send_message"));
        assert!(BLOCKED_TOOLS.contains(&"execute_code"));
    }

    #[test]
    fn test_delegation_config_defaults() {
        let cfg = DelegationConfig::default();
        assert_eq!(cfg.max_depth, 3);
        assert_eq!(cfg.max_children, 3);
    }

    #[test]
    fn test_delegation_tracker_clone() {
        let tracker = DelegationTracker::new(0);
        let cloned = tracker.clone();
        assert_eq!(cloned.depth, tracker.depth);
        assert_eq!(cloned.max_children, tracker.max_children);
    }

    #[tokio::test]
    async fn test_spawn_uses_registry_for_routing_limits() {
        use std::time::Duration;

        let tool = DelegateTool::new(0);
        let registry = Arc::clone(tool.registry());
        let context = ToolContext::new("user", "parent-session");

        let args = serde_json::json!({
            "action": "spawn",
            "task": { "prompt": "test task" }
        });

        let result = tool.execute(args, &context).await.expect("execute spawn");
        assert!(result.success, "spawn should succeed: {:?}", result);

        let child_id = result
            .data
            .as_ref()
            .and_then(|d| d.get("child_id"))
            .and_then(|v| v.as_str())
            .expect("child_id in result")
            .to_string();

        // The registry must know about the same run id.
        assert!(
            registry.get_run(&child_id).await.is_some(),
            "registry should contain run with child_id"
        );

        // Cancel should kill the registry run.
        let cancel_args = serde_json::json!({
            "action": "cancel",
            "child_id": child_id
        });
        let cancel_result = tool
            .execute(cancel_args, &context)
            .await
            .expect("execute cancel");
        assert!(cancel_result.success);

        let run = registry
            .get_run(&child_id)
            .await
            .expect("run still in registry");
        assert!(matches!(run.status, crate::agent::SubagentStatus::Killed));
    }
}
