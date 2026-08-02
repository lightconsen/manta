//! ACP session persistence.
//!
//! CRUD for `acp_sessions` rows: save, load, list, and delete Agent Client
//! Protocol sessions and their subagent memberships.

use chrono::{DateTime, Utc};
use sqlx::Row;
use tracing::debug;

use crate::error::{Result, SyscityError};

use super::SessionStore;

impl SessionStore {
    /// Persist an ACP session.
    pub async fn save_acp_session(
        &self,
        session_id: &str,
        parent_id: &str,
        subagent_ids: &[String],
        created_at: DateTime<Utc>,
    ) -> Result<()> {
        let ids_json = serde_json::to_string(subagent_ids).unwrap_or_else(|_| "[]".to_string());
        sqlx::query(
            r#"
            INSERT INTO acp_sessions (session_id, parent_id, subagent_ids, created_at)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(session_id) DO UPDATE SET
                parent_id = excluded.parent_id,
                subagent_ids = excluded.subagent_ids,
                created_at = excluded.created_at
            "#,
        )
        .bind(session_id)
        .bind(parent_id)
        .bind(ids_json)
        .bind(created_at.timestamp_millis())
        .execute(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to save ACP session".to_string(),
            details: e.to_string(),
        })?;

        debug!("ACP session saved: {}", session_id);
        Ok(())
    }

    /// Load a single ACP session.
    pub async fn load_acp_session(
        &self,
        session_id: &str,
    ) -> Result<Option<(String, Vec<String>, DateTime<Utc>)>> {
        let row = sqlx::query(
            "SELECT parent_id, subagent_ids, created_at FROM acp_sessions WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to load ACP session".to_string(),
            details: e.to_string(),
        })?;

        Ok(row.map(|r| {
            let parent_id: String = r.get("parent_id");
            let ids_json: String = r.get("subagent_ids");
            let subagent_ids: Vec<String> = serde_json::from_str(&ids_json).unwrap_or_default();
            let created_at = DateTime::from_timestamp_millis(r.get::<i64, _>("created_at"))
                .unwrap_or_else(Utc::now);
            (parent_id, subagent_ids, created_at)
        }))
    }

    /// List all persisted ACP sessions.
    pub async fn list_acp_sessions(
        &self,
    ) -> Result<Vec<(String, String, Vec<String>, DateTime<Utc>)>> {
        let rows = sqlx::query(
            "SELECT session_id, parent_id, subagent_ids, created_at FROM acp_sessions ORDER BY \
             created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to list ACP sessions".to_string(),
            details: e.to_string(),
        })?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let session_id: String = r.get("session_id");
                let parent_id: String = r.get("parent_id");
                let ids_json: String = r.get("subagent_ids");
                let subagent_ids: Vec<String> = serde_json::from_str(&ids_json).unwrap_or_default();
                let created_at = DateTime::from_timestamp_millis(r.get::<i64, _>("created_at"))
                    .unwrap_or_else(Utc::now);
                (session_id, parent_id, subagent_ids, created_at)
            })
            .collect())
    }

    /// Delete a persisted ACP session.
    pub async fn delete_acp_session(&self, session_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM acp_sessions WHERE session_id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SyscityError::Storage {
                context: "Failed to delete ACP session".to_string(),
                details: e.to_string(),
            })?;

        debug!("ACP session deleted: {}", session_id);
        Ok(())
    }
}
