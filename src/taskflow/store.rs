//! SQLite checkpoint store for TaskFlow durable execution
//!
//! Persists execution state to SQLite for crash recovery.

use super::state::{TaskFlowCheckpoint, TaskFlowState};
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use std::time::Duration;
use tracing::{debug, info, instrument};

/// SQLite-backed checkpoint store
#[derive(Debug, Clone)]
pub struct CheckpointStore {
    pool: Pool<Sqlite>,
}

impl CheckpointStore {
    /// Create a new checkpoint store
    pub async fn new(database_url: &str) -> crate::Result<Self> {
        info!("Initializing TaskFlow checkpoint store");

        if database_url.starts_with("sqlite://") && !database_url.contains(":memory:") {
            let path_str = database_url
                .strip_prefix("sqlite://")
                .unwrap_or(database_url);
            let path = std::path::Path::new(path_str);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    crate::error::SyscityError::Storage {
                        context: format!("Failed to create checkpoint directory: {:?}", parent),
                        details: e.to_string(),
                    }
                })?;
            }
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(30))
            .connect(database_url)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to connect to checkpoint database".to_string(),
                details: e.to_string(),
            })?;

        let store = Self { pool };
        store.init_schema().await?;
        Ok(store)
    }

    /// Create the checkpoint table
    async fn init_schema(&self) -> crate::Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS taskflow_checkpoints (
                id TEXT PRIMARY KEY,
                flow_id TEXT NOT NULL,
                state TEXT NOT NULL,
                current_task_index INTEGER NOT NULL DEFAULT 0,
                completed_tasks TEXT NOT NULL DEFAULT '[]',
                task_outputs TEXT NOT NULL DEFAULT '{}',
                variables TEXT NOT NULL DEFAULT '{}',
                retry_count INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                created_at TEXT NOT NULL,
                goal TEXT NOT NULL DEFAULT '',
                plan_json TEXT NOT NULL DEFAULT '{}',
                sequence INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to create checkpoint table".to_string(),
            details: e.to_string(),
        })?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_checkpoints_flow
            ON taskflow_checkpoints(flow_id)
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to create checkpoint index".to_string(),
            details: e.to_string(),
        })?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_checkpoints_sequence
            ON taskflow_checkpoints(flow_id, sequence DESC)
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to create sequence index".to_string(),
            details: e.to_string(),
        })?;

        Ok(())
    }

    /// Save a checkpoint
    #[instrument(skip(self, checkpoint))]
    pub async fn save(&self, checkpoint: &TaskFlowCheckpoint) -> crate::Result<()> {
        debug!(
            "Saving checkpoint {} for flow {} (seq {})",
            checkpoint.id, checkpoint.flow_id, checkpoint.sequence
        );

        let completed_json =
            serde_json::to_string(&checkpoint.completed_tasks).unwrap_or_else(|_| "[]".to_string());
        let outputs_json =
            serde_json::to_string(&checkpoint.task_outputs).unwrap_or_else(|_| "{}".to_string());
        let vars_json =
            serde_json::to_string(&checkpoint.variables).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO taskflow_checkpoints
            (id, flow_id, state, current_task_index, completed_tasks, task_outputs,
             variables, retry_count, error, created_at, goal, plan_json, sequence)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            "#,
        )
        .bind(&checkpoint.id)
        .bind(&checkpoint.flow_id)
        .bind(checkpoint.state.to_string())
        .bind(checkpoint.current_task_index as i64)
        .bind(&completed_json)
        .bind(&outputs_json)
        .bind(&vars_json)
        .bind(checkpoint.retry_count as i64)
        .bind(&checkpoint.error)
        .bind(checkpoint.created_at.to_rfc3339())
        .bind(&checkpoint.goal)
        .bind(&checkpoint.plan_json)
        .bind(checkpoint.sequence as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to save checkpoint".to_string(),
            details: e.to_string(),
        })?;

        info!(
            "Checkpoint saved: {} for flow {} (seq {})",
            checkpoint.id, checkpoint.flow_id, checkpoint.sequence
        );
        Ok(())
    }

    /// Load the latest checkpoint for a flow
    #[instrument(skip(self))]
    pub async fn load_latest(&self, flow_id: &str) -> crate::Result<Option<TaskFlowCheckpoint>> {
        let row = sqlx::query(
            r#"
            SELECT id, flow_id, state, current_task_index, completed_tasks,
                   task_outputs, variables, retry_count, error, created_at,
                   goal, plan_json, sequence
            FROM taskflow_checkpoints
            WHERE flow_id = ?1
            ORDER BY sequence DESC
            LIMIT 1
            "#,
        )
        .bind(flow_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to load checkpoint".to_string(),
            details: e.to_string(),
        })?;

        match row {
            Some(r) => {
                let checkpoint = self.row_to_checkpoint(r).await?;
                info!(
                    "Loaded checkpoint {} for flow {} (seq {}, state {})",
                    checkpoint.id, checkpoint.flow_id, checkpoint.sequence, checkpoint.state
                );
                Ok(Some(checkpoint))
            }
            None => {
                debug!("No checkpoint found for flow {}", flow_id);
                Ok(None)
            }
        }
    }

    /// Load checkpoint by ID
    pub async fn load_by_id(&self, id: &str) -> crate::Result<Option<TaskFlowCheckpoint>> {
        let row = sqlx::query(
            r#"
            SELECT id, flow_id, state, current_task_index, completed_tasks,
                   task_outputs, variables, retry_count, error, created_at,
                   goal, plan_json, sequence
            FROM taskflow_checkpoints
            WHERE id = ?1
            LIMIT 1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to load checkpoint by ID".to_string(),
            details: e.to_string(),
        })?;

        match row {
            Some(r) => Ok(Some(self.row_to_checkpoint(r).await?)),
            None => Ok(None),
        }
    }

    /// List all checkpoints for a flow
    pub async fn list_checkpoints(&self, flow_id: &str) -> crate::Result<Vec<TaskFlowCheckpoint>> {
        let rows = sqlx::query(
            r#"
            SELECT id, flow_id, state, current_task_index, completed_tasks,
                   task_outputs, variables, retry_count, error, created_at,
                   goal, plan_json, sequence
            FROM taskflow_checkpoints
            WHERE flow_id = ?1
            ORDER BY sequence ASC
            "#,
        )
        .bind(flow_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to list checkpoints".to_string(),
            details: e.to_string(),
        })?;

        let mut checkpoints = Vec::new();
        for row in rows {
            checkpoints.push(self.row_to_checkpoint(row).await?);
        }

        Ok(checkpoints)
    }

    /// Delete all checkpoints for a flow
    pub async fn delete_flow(&self, flow_id: &str) -> crate::Result<u64> {
        let result = sqlx::query("DELETE FROM taskflow_checkpoints WHERE flow_id = ?1")
            .bind(flow_id)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to delete checkpoints".to_string(),
                details: e.to_string(),
            })?;

        info!("Deleted {} checkpoints for flow {}", result.rows_affected(), flow_id);
        Ok(result.rows_affected())
    }

    /// Prune old checkpoints, keeping only the latest N per flow
    pub async fn prune(&self, keep_per_flow: usize) -> crate::Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM taskflow_checkpoints
            WHERE id NOT IN (
                SELECT id FROM (
                    SELECT id, ROW_NUMBER() OVER (PARTITION BY flow_id ORDER BY sequence DESC) as rn
                    FROM taskflow_checkpoints
                ) WHERE rn <= ?1
            )
            "#,
        )
        .bind(keep_per_flow as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to prune checkpoints".to_string(),
            details: e.to_string(),
        })?;

        if result.rows_affected() > 0 {
            info!("Pruned {} old checkpoints", result.rows_affected());
        }
        Ok(result.rows_affected())
    }

    /// Convert a database row to a checkpoint
    async fn row_to_checkpoint(
        &self,
        row: sqlx::sqlite::SqliteRow,
    ) -> crate::Result<TaskFlowCheckpoint> {
        let state_str: String =
            row.try_get("state")
                .map_err(|e| crate::error::SyscityError::Storage {
                    context: "Failed to read state column".to_string(),
                    details: e.to_string(),
                })?;

        let state = match state_str.as_str() {
            "idle" => TaskFlowState::Idle,
            "running" => TaskFlowState::Running,
            "paused" => TaskFlowState::Paused,
            "failed" => TaskFlowState::Failed,
            "completed" => TaskFlowState::Completed,
            "recovering" => TaskFlowState::Recovering,
            _ => TaskFlowState::Idle,
        };

        let created_at_str: String =
            row.try_get("created_at")
                .map_err(|e| crate::error::SyscityError::Storage {
                    context: "Failed to read created_at column".to_string(),
                    details: e.to_string(),
                })?;

        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
            .map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to parse created_at".to_string(),
                details: e.to_string(),
            })?
            .with_timezone(&chrono::Utc);

        let completed_tasks: Vec<String> = serde_json::from_str(
            row.try_get::<String, _>("completed_tasks")
                .unwrap_or_default()
                .as_str(),
        )
        .unwrap_or_default();

        let task_outputs: std::collections::HashMap<String, String> = serde_json::from_str(
            row.try_get::<String, _>("task_outputs")
                .unwrap_or_default()
                .as_str(),
        )
        .unwrap_or_default();

        let variables: std::collections::HashMap<String, String> = serde_json::from_str(
            row.try_get::<String, _>("variables")
                .unwrap_or_default()
                .as_str(),
        )
        .unwrap_or_default();

        Ok(TaskFlowCheckpoint {
            id: row.try_get("id").unwrap_or_default(),
            flow_id: row.try_get("flow_id").unwrap_or_default(),
            state,
            current_task_index: row.try_get::<i64, _>("current_task_index").unwrap_or(0) as usize,
            completed_tasks,
            task_outputs,
            variables,
            retry_count: row.try_get::<i64, _>("retry_count").unwrap_or(0) as u32,
            error: row.try_get("error").ok(),
            created_at,
            goal: row.try_get("goal").unwrap_or_default(),
            plan_json: row.try_get("plan_json").unwrap_or_default(),
            sequence: row.try_get::<i64, _>("sequence").unwrap_or(0) as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_store() -> CheckpointStore {
        CheckpointStore::new("sqlite::memory:").await.unwrap()
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let store = create_test_store().await;
        let mut cp = TaskFlowCheckpoint::new("flow-1", "Build app");
        cp.complete_task("task_1", "output_1");
        cp.set_variable("key", "value");

        store.save(&cp).await.unwrap();

        let loaded = store.load_latest("flow-1").await.unwrap().unwrap();
        assert_eq!(loaded.flow_id, "flow-1");
        assert_eq!(loaded.goal, "Build app");
        assert_eq!(loaded.current_task_index, 1);
        assert_eq!(loaded.completed_tasks, vec!["task_1"]);
        assert_eq!(loaded.variables.get("key"), Some(&"value".to_string()));
    }

    #[tokio::test]
    async fn test_load_not_found() {
        let store = create_test_store().await;
        let loaded = store.load_latest("nonexistent").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_multiple_checkpoints() {
        let store = create_test_store().await;
        let mut cp = TaskFlowCheckpoint::new("flow-1", "g");
        cp.complete_task("t1", "done");
        store.save(&cp).await.unwrap();

        let mut cp2 = cp.successor();
        cp2.complete_task("t2", "done2");
        store.save(&cp2).await.unwrap();

        let all = store.list_checkpoints("flow-1").await.unwrap();
        assert_eq!(all.len(), 2);

        let latest = store.load_latest("flow-1").await.unwrap().unwrap();
        assert_eq!(latest.sequence, 1);
        assert_eq!(latest.current_task_index, 2);
    }

    #[tokio::test]
    async fn test_delete_flow() {
        let store = create_test_store().await;
        let cp = TaskFlowCheckpoint::new("flow-del", "g");
        store.save(&cp).await.unwrap();

        let deleted = store.delete_flow("flow-del").await.unwrap();
        assert_eq!(deleted, 1);

        let loaded = store.load_latest("flow-del").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_prune() {
        let store = create_test_store().await;
        let mut cp = TaskFlowCheckpoint::new("flow-1", "g");
        for _ in 0..5 {
            cp = cp.successor();
            store.save(&cp).await.unwrap();
        }

        let pruned = store.prune(2).await.unwrap();
        assert_eq!(pruned, 3); // 5 total, keep 2 latest

        let remaining = store.list_checkpoints("flow-1").await.unwrap();
        assert_eq!(remaining.len(), 2);
    }

    #[tokio::test]
    async fn test_load_by_id() {
        let store = create_test_store().await;
        let cp = TaskFlowCheckpoint::new("flow-1", "g");
        store.save(&cp).await.unwrap();

        let loaded = store.load_by_id(&cp.id).await.unwrap().unwrap();
        assert_eq!(loaded.id, cp.id);
    }

    #[tokio::test]
    async fn test_checkpoint_with_error() {
        let store = create_test_store().await;
        let mut cp = TaskFlowCheckpoint::new("flow-err", "g");
        cp.record_failure("network timeout");
        store.save(&cp).await.unwrap();

        let loaded = store.load_latest("flow-err").await.unwrap().unwrap();
        assert_eq!(loaded.state, TaskFlowState::Failed);
        assert_eq!(loaded.error, Some("network timeout".to_string()));
    }
}
