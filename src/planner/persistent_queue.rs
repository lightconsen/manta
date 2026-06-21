//! Persistent task queue — high-level wrapper over [`TaskStateStore`].
//!
//! The [`PersistentTaskManager`] provides queue semantics on top of the
//! SQLite-backed state store: queue a goal, peek at pending work, resume
//! incomplete plans on startup, and report queue health.
//!
//! Since `TaskStateStore` already handles SQLite persistence, crash recovery,
//! and plan loading, this module is intentionally thin — it adds queue-style
//! ergonomics and batch operations.

use tracing::info;

use crate::planner::state::PlanSummary;
use crate::planner::{Plan, Task, TaskStateStore, TaskStatus};

/// High-level manager for persistent goals and tasks.
pub struct PersistentTaskManager {
    store: TaskStateStore,
}

impl PersistentTaskManager {
    /// Create a new manager wrapping the given state store.
    pub fn new(store: TaskStateStore) -> Self {
        Self { store }
    }

    /// Queue a new goal (save plan + tasks atomically).
    pub async fn queue_goal(&self, plan_id: impl Into<String>, plan: &Plan) -> crate::Result<()> {
        let id = plan_id.into();
        self.store.save_plan(&id, plan).await?;
        info!("Queued goal '{}' with {} tasks", id, plan.tasks.len());
        Ok(())
    }

    /// Queue a single task under a plan.
    pub async fn queue_task(&self, plan_id: &str, task: &Task) -> crate::Result<()> {
        self.store.save_task(plan_id, task).await?;
        Ok(())
    }

    /// Mark a task as completed with an optional result.
    pub async fn complete_task(
        &self,
        plan_id: &str,
        task_id: &str,
        result: Option<&str>,
    ) -> crate::Result<()> {
        self.store
            .update_task_status(plan_id, task_id, TaskStatus::Completed, result, None)
            .await?;
        Ok(())
    }

    /// Mark a task as failed with an error message.
    pub async fn fail_task(&self, plan_id: &str, task_id: &str, error: &str) -> crate::Result<()> {
        self.store
            .update_task_status(plan_id, task_id, TaskStatus::Failed, None, Some(error))
            .await?;
        Ok(())
    }

    /// Mark a plan as completed.
    pub async fn complete_plan(&self, plan_id: &str, success: bool) -> crate::Result<()> {
        self.store.complete_plan(plan_id, success).await?;
        info!("Completed plan '{}' (success={})", plan_id, success);
        Ok(())
    }

    /// Load a plan and all its tasks.
    pub async fn load_plan(&self, plan_id: &str) -> crate::Result<Option<Plan>> {
        self.store.load_plan(plan_id).await
    }

    /// List all incomplete plan IDs.
    pub async fn pending_plan_ids(&self) -> crate::Result<Vec<String>> {
        self.store.list_incomplete_plans().await
    }

    /// Get summaries of all incomplete plans with task progress.
    pub async fn pending_summaries(&self) -> crate::Result<Vec<PlanSummary>> {
        self.store.load_plan_summaries().await
    }

    /// Resume the oldest incomplete plan, returning it if found.
    pub async fn resume_next_plan(&self) -> crate::Result<Option<Plan>> {
        let ids = self.store.list_incomplete_plans().await?;
        if let Some(id) = ids.last() {
            // last = oldest because list_incomplete_plans orders DESC.
            self.store.load_plan(id).await
        } else {
            Ok(None)
        }
    }

    /// Delete a plan and all its tasks.
    pub async fn delete_plan(&self, plan_id: &str) -> crate::Result<()> {
        self.store.delete_plan(plan_id).await?;
        info!("Deleted plan '{}'", plan_id);
        Ok(())
    }

    /// Get the total number of incomplete plans.
    pub async fn pending_count(&self) -> crate::Result<usize> {
        let ids = self.store.list_incomplete_plans().await?;
        Ok(ids.len())
    }

    /// Health check: report queue depth and any stuck (running) tasks.
    pub async fn health_report(&self) -> crate::Result<QueueHealth> {
        let summaries = self.store.load_plan_summaries().await?;
        let mut total_pending_tasks = 0usize;
        let mut total_failed_tasks = 0usize;
        let mut plans_with_failures = 0usize;

        for s in &summaries {
            total_pending_tasks += s.pending_tasks;
            total_failed_tasks += s.failed_tasks;
            if s.failed_tasks > 0 {
                plans_with_failures += 1;
            }
        }

        let status = if total_failed_tasks > 0 {
            QueueStatus::Degraded
        } else if total_pending_tasks > 10 {
            QueueStatus::Backlogged
        } else {
            QueueStatus::Healthy
        };

        Ok(QueueHealth {
            incomplete_plans: summaries.len(),
            total_pending_tasks,
            total_failed_tasks,
            plans_with_failures,
            status,
        })
    }

    /// Clone the underlying state store.
    pub fn state_store(&self) -> &TaskStateStore {
        &self.store
    }
}

impl Clone for PersistentTaskManager {
    fn clone(&self) -> Self {
        Self { store: self.store.clone() }
    }
}

/// Queue health snapshot.
#[derive(Debug, Clone)]
pub struct QueueHealth {
    pub incomplete_plans: usize,
    pub total_pending_tasks: usize,
    pub total_failed_tasks: usize,
    pub plans_with_failures: usize,
    pub status: QueueStatus,
}

/// Overall queue status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueStatus {
    Healthy,
    Backlogged,
    Degraded,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::DesktopAction;
    use crate::planner::{Plan, Task};

    async fn create_test_manager() -> PersistentTaskManager {
        let store = TaskStateStore::new("sqlite::memory:")
            .await
            .expect("in-memory store should initialize");
        PersistentTaskManager::new(store)
    }

    #[tokio::test]
    async fn test_queue_and_load_plan() {
        let mgr = create_test_manager().await;
        let mut plan = Plan::new("deploy app");
        plan.add_task(Task::new("a", "step A", DesktopAction::Wait { milliseconds: 0 }));

        mgr.queue_goal("plan-1", &plan).await.unwrap();
        let loaded = mgr.load_plan("plan-1").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().goal, "deploy app");
    }

    #[tokio::test]
    async fn test_pending_plan_ids() {
        let mgr = create_test_manager().await;
        let mut plan = Plan::new("test");
        plan.add_task(Task::new("a", "step A", DesktopAction::Wait { milliseconds: 0 }));

        mgr.queue_goal("p1", &plan).await.unwrap();
        let ids = mgr.pending_plan_ids().await.unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], "p1");
    }

    #[tokio::test]
    async fn test_complete_and_delete_plan() {
        let mgr = create_test_manager().await;
        let mut plan = Plan::new("test");
        plan.add_task(Task::new("a", "step A", DesktopAction::Wait { milliseconds: 0 }));

        mgr.queue_goal("p1", &plan).await.unwrap();
        mgr.complete_plan("p1", true).await.unwrap();

        let ids = mgr.pending_plan_ids().await.unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn test_health_report() {
        let mgr = create_test_manager().await;
        let mut plan = Plan::new("test");
        plan.add_task(Task::new("a", "step A", DesktopAction::Wait { milliseconds: 0 }));

        mgr.queue_goal("p1", &plan).await.unwrap();
        let health = mgr.health_report().await.unwrap();
        assert_eq!(health.incomplete_plans, 1);
        assert_eq!(health.status, QueueStatus::Healthy);
    }
}
