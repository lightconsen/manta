//! Channel State Persistence
//!
//! Persists channel state (offsets, session mappings) to SQLite for recovery.

use crate::error::{MantaError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Sqlite};
use std::collections::HashMap;
use tracing::info;

/// Persisted channel state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelState {
    /// Channel name (e.g., "telegram", "discord")
    pub channel_name: String,
    /// Account/bot identifier
    pub account_id: Option<String>,
    /// Update offset for polling (e.g., Telegram update_id)
    pub update_offset: Option<i64>,
    /// Session ID mappings (conversation_id -> session_id)
    pub session_mappings: HashMap<String, String>,
    /// Last activity timestamp
    pub last_activity: DateTime<Utc>,
    /// Additional channel-specific data (JSON)
    pub extra_data: Option<String>,
}

impl ChannelState {
    /// Create new channel state
    pub fn new(channel_name: impl Into<String>) -> Self {
        Self {
            channel_name: channel_name.into(),
            account_id: None,
            update_offset: None,
            session_mappings: HashMap::new(),
            last_activity: Utc::now(),
            extra_data: None,
        }
    }

    /// Set account ID
    pub fn with_account_id(mut self, account_id: impl Into<String>) -> Self {
        self.account_id = Some(account_id.into());
        self
    }

    /// Set update offset
    pub fn with_offset(mut self, offset: i64) -> Self {
        self.update_offset = Some(offset);
        self
    }

    /// Add session mapping
    pub fn add_session_mapping(
        &mut self,
        conversation_id: impl Into<String>,
        session_id: impl Into<String>,
    ) {
        self.session_mappings
            .insert(conversation_id.into(), session_id.into());
        self.last_activity = Utc::now();
    }
}

/// SQLite-backed channel state store
pub struct ChannelStateStore {
    db: Pool<Sqlite>,
}

impl ChannelStateStore {
    /// Create a new state store
    pub fn new(db: Pool<Sqlite>) -> Self {
        Self { db }
    }

    /// Initialize the database schema
    pub async fn init_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS channel_states (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_name TEXT NOT NULL,
                account_id TEXT NOT NULL DEFAULT '',
                update_offset INTEGER,
                session_mappings TEXT NOT NULL DEFAULT '{}',
                extra_data TEXT,
                last_activity DATETIME NOT NULL,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(channel_name, account_id)
            );

            CREATE INDEX IF NOT EXISTS idx_channel_states_name
                ON channel_states(channel_name);

            CREATE TABLE IF NOT EXISTS channel_health_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_name TEXT NOT NULL,
                status TEXT NOT NULL,
                message TEXT,
                recorded_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_channel_health_log_name
                ON channel_health_log(channel_name);
            "#,
        )
        .execute(&self.db)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to create channel state schema".to_string(),
            details: e.to_string(),
        })?;

        info!("Channel state schema initialized");
        Ok(())
    }

    /// Save channel state
    pub async fn save_state(&self, state: &ChannelState) -> Result<()> {
        let session_mappings_json =
            serde_json::to_string(&state.session_mappings).map_err(|e| {
                MantaError::Internal(format!("Failed to serialize session mappings: {}", e))
            })?;

        sqlx::query(
            r#"
            INSERT INTO channel_states
                (channel_name, account_id, update_offset, session_mappings, extra_data, last_activity)
            VALUES
                (?, ?, ?, ?, ?, ?)
            ON CONFLICT(channel_name, account_id) DO UPDATE SET
                update_offset = excluded.update_offset,
                session_mappings = excluded.session_mappings,
                extra_data = excluded.extra_data,
                last_activity = excluded.last_activity
            "#,
        )
        .bind(&state.channel_name)
        .bind(state.account_id.as_deref().unwrap_or(""))
        .bind(state.update_offset)
        .bind(&session_mappings_json)
        .bind(&state.extra_data)
        .bind(state.last_activity)
        .execute(&self.db)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to save channel state".to_string(),
            details: e.to_string(),
        })?;

        Ok(())
    }

    /// Load channel state
    pub async fn load_state(
        &self,
        channel_name: &str,
        account_id: Option<&str>,
    ) -> Result<Option<ChannelState>> {
        let row = sqlx::query_as::<
            _,
            (String, String, Option<i64>, String, Option<String>, DateTime<Utc>),
        >(
            r#"
            SELECT
                channel_name,
                account_id,
                update_offset,
                session_mappings,
                extra_data,
                last_activity
            FROM channel_states
            WHERE channel_name = ? AND account_id = ?
            LIMIT 1
            "#,
        )
        .bind(channel_name)
        .bind(account_id.unwrap_or(""))
        .fetch_optional(&self.db)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to load channel state".to_string(),
            details: e.to_string(),
        })?;

        match row {
            Some((name, account, offset, mappings, extra, activity)) => {
                let session_mappings: HashMap<String, String> = serde_json::from_str(&mappings)
                    .map_err(|e| {
                        MantaError::Internal(format!(
                            "Failed to deserialize session mappings: {}",
                            e
                        ))
                    })?;

                // Normalize empty string sentinel back to None
                let account_id = if account.is_empty() {
                    None
                } else {
                    Some(account)
                };

                Ok(Some(ChannelState {
                    channel_name: name,
                    account_id,
                    update_offset: offset,
                    session_mappings,
                    last_activity: activity,
                    extra_data: extra,
                }))
            }
            None => Ok(None),
        }
    }

    /// Delete channel state
    pub async fn delete_state(&self, channel_name: &str, account_id: Option<&str>) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM channel_states
            WHERE channel_name = ? AND account_id = ?
            "#,
        )
        .bind(channel_name)
        .bind(account_id.unwrap_or(""))
        .execute(&self.db)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to delete channel state".to_string(),
            details: e.to_string(),
        })?;

        Ok(())
    }

    /// List all saved states for a channel
    pub async fn list_states(&self, channel_name: &str) -> Result<Vec<ChannelState>> {
        let rows = sqlx::query_as::<
            _,
            (String, String, Option<i64>, String, Option<String>, DateTime<Utc>),
        >(
            r#"
            SELECT
                channel_name,
                account_id,
                update_offset,
                session_mappings,
                extra_data,
                last_activity
            FROM channel_states
            WHERE channel_name = ?
            ORDER BY last_activity DESC
            "#,
        )
        .bind(channel_name)
        .fetch_all(&self.db)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to list channel states".to_string(),
            details: e.to_string(),
        })?;

        let mut states = Vec::new();
        for (name, account, offset, mappings, extra, activity) in rows {
            let session_mappings: HashMap<String, String> = serde_json::from_str(&mappings)
                .map_err(|e| {
                    MantaError::Internal(format!("Failed to deserialize session mappings: {}", e))
                })?;
            let account_id = if account.is_empty() {
                None
            } else {
                Some(account)
            };

            states.push(ChannelState {
                channel_name: name,
                account_id,
                update_offset: offset,
                session_mappings,
                last_activity: activity,
                extra_data: extra,
            });
        }

        Ok(states)
    }

    /// Log health status change
    pub async fn log_health_status(
        &self,
        channel_name: &str,
        status: &str,
        message: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO channel_health_log (channel_name, status, message)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(channel_name)
        .bind(status)
        .bind(message)
        .execute(&self.db)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to log health status".to_string(),
            details: e.to_string(),
        })?;

        Ok(())
    }

    /// Get health log for a channel
    pub async fn get_health_log(
        &self,
        channel_name: &str,
        limit: i32,
    ) -> Result<Vec<(String, Option<String>, DateTime<Utc>)>> {
        let rows = sqlx::query_as::<_, (String, Option<String>, DateTime<Utc>)>(
            r#"
            SELECT status, message, recorded_at
            FROM channel_health_log
            WHERE channel_name = ?
            ORDER BY recorded_at DESC
            LIMIT ?
            "#,
        )
        .bind(channel_name)
        .bind(limit)
        .fetch_all(&self.db)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to get health log".to_string(),
            details: e.to_string(),
        })?;

        Ok(rows)
    }

    /// Clean up old health log entries
    pub async fn cleanup_health_log(&self, older_than_days: i32) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM channel_health_log
            WHERE recorded_at < datetime('now', '-' || ? || ' days')
            "#,
        )
        .bind(older_than_days)
        .execute(&self.db)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to cleanup health log".to_string(),
            details: e.to_string(),
        })?;

        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_pool() -> Pool<Sqlite> {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("Failed to create test pool");
        pool
    }

    #[tokio::test]
    async fn test_channel_state_new() {
        let state = ChannelState::new("telegram");

        assert_eq!(state.channel_name, "telegram");
        assert!(state.account_id.is_none());
        assert!(state.update_offset.is_none());
        assert!(state.session_mappings.is_empty());
    }

    #[tokio::test]
    async fn test_channel_state_builder() {
        let state = ChannelState::new("telegram")
            .with_account_id("bot123")
            .with_offset(12345);

        assert_eq!(state.channel_name, "telegram");
        assert_eq!(state.account_id, Some("bot123".to_string()));
        assert_eq!(state.update_offset, Some(12345));
    }

    #[tokio::test]
    async fn test_channel_state_session_mapping() {
        let mut state = ChannelState::new("telegram");
        state.add_session_mapping("chat_1", "session_1");
        state.add_session_mapping("chat_2", "session_2");

        assert_eq!(state.session_mappings.get("chat_1"), Some(&"session_1".to_string()));
        assert_eq!(state.session_mappings.get("chat_2"), Some(&"session_2".to_string()));
    }

    #[tokio::test]
    async fn test_state_store_init_schema() {
        let pool = create_test_pool().await;
        let store = ChannelStateStore::new(pool);

        assert!(store.init_schema().await.is_ok());
    }

    #[tokio::test]
    async fn test_state_store_save_and_load() {
        let pool = create_test_pool().await;
        let store = ChannelStateStore::new(pool);
        store.init_schema().await.unwrap();

        let mut state = ChannelState::new("telegram")
            .with_account_id("bot123")
            .with_offset(12345);
        state.add_session_mapping("chat_1", "session_abc");

        // Save state
        store.save_state(&state).await.unwrap();

        // Load state
        let loaded = store
            .load_state("telegram", Some("bot123"))
            .await
            .unwrap()
            .expect("State should exist");

        assert_eq!(loaded.channel_name, "telegram");
        assert_eq!(loaded.account_id, Some("bot123".to_string()));
        assert_eq!(loaded.update_offset, Some(12345));
        assert_eq!(loaded.session_mappings.get("chat_1"), Some(&"session_abc".to_string()));
    }

    #[tokio::test]
    async fn test_state_store_update_existing() {
        let pool = create_test_pool().await;
        let store = ChannelStateStore::new(pool);
        store.init_schema().await.unwrap();

        // Save initial state
        let state = ChannelState::new("telegram").with_offset(100);
        store.save_state(&state).await.unwrap();

        // Update with new offset
        let updated_state = ChannelState::new("telegram").with_offset(200);
        store.save_state(&updated_state).await.unwrap();

        // Load and verify
        let loaded = store.load_state("telegram", None).await.unwrap().unwrap();
        assert_eq!(loaded.update_offset, Some(200));
    }

    #[tokio::test]
    async fn test_state_store_delete() {
        let pool = create_test_pool().await;
        let store = ChannelStateStore::new(pool);
        store.init_schema().await.unwrap();

        let state = ChannelState::new("telegram").with_account_id("bot123");
        store.save_state(&state).await.unwrap();

        // Verify it exists
        assert!(store
            .load_state("telegram", Some("bot123"))
            .await
            .unwrap()
            .is_some());

        // Delete it
        store
            .delete_state("telegram", Some("bot123"))
            .await
            .unwrap();

        // Verify it's gone
        assert!(store
            .load_state("telegram", Some("bot123"))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_health_log() {
        let pool = create_test_pool().await;
        let store = ChannelStateStore::new(pool);
        store.init_schema().await.unwrap();

        // Log some health changes
        store
            .log_health_status("telegram", "healthy", None)
            .await
            .unwrap();
        store
            .log_health_status("telegram", "degraded", Some("Slow response"))
            .await
            .unwrap();
        store
            .log_health_status("telegram", "healthy", None)
            .await
            .unwrap();

        // Get log
        let log = store.get_health_log("telegram", 10).await.unwrap();
        assert_eq!(log.len(), 3);

        // Verify order (most recent first)
        assert_eq!(log[0].0, "healthy");
        assert_eq!(log[1].0, "degraded");
        assert_eq!(log[1].1, Some("Slow response".to_string()));
    }
}
