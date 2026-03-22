//! SQLite Session Storage
//!
//! Provides persistent session storage using SQLite instead of in-memory HashMaps.
//! This gives us ACID guarantees, automatic crash recovery, and simpler querying.

use crate::error::{MantaError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

/// Session metadata for querying
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Session ID (UUID)
    pub session_id: String,
    /// Agent ID ("main", "coder", etc.)
    pub agent_id: String,
    /// Channel ("discord", "telegram", etc.)
    pub channel: String,
    /// Channel-specific ID (user ID, channel ID)
    pub channel_id: String,
    /// Session creation time
    pub created_at: DateTime<Utc>,
    /// Last activity time
    pub last_activity: DateTime<Utc>,
    /// Whether session is active
    pub is_active: bool,
    /// Message count
    #[serde(default)]
    pub message_count: usize,
}

impl SessionMetadata {
    /// Create new session metadata
    pub fn new(
        session_id: impl Into<String>,
        agent_id: impl Into<String>,
        channel: impl Into<String>,
        channel_id: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            session_id: session_id.into(),
            agent_id: agent_id.into(),
            channel: channel.into(),
            channel_id: channel_id.into(),
            created_at: now,
            last_activity: now,
            is_active: true,
            message_count: 0,
        }
    }

    /// Update last activity
    pub fn touch(&mut self) {
        self.last_activity = Utc::now();
    }
}

/// Persisted session data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSession {
    /// Session ID
    pub id: String,
    /// Session metadata
    pub metadata: SessionMetadata,
    /// Serialized session state (JSON)
    pub state_json: String,
    /// Message count
    pub message_count: i64,
}

/// Session storage using SQLite
#[derive(Debug, Clone)]
pub struct SessionStore {
    pool: Pool<Sqlite>,
    /// In-memory cache of active sessions (session_id -> last_accessed)
    cache: Arc<RwLock<lru::LruCache<String, DateTime<Utc>>>>,
}

impl SessionStore {
    /// Create a new session store
    #[instrument(skip(database_url))]
    pub async fn new(database_url: &str) -> Result<Self> {
        info!("Initializing SQLite session store");

        let pool = SqlitePoolOptions::new()
            .max_connections(10)
            .min_connections(2)
            .acquire_timeout(Duration::from_secs(30))
            .idle_timeout(Duration::from_secs(600))
            .max_lifetime(Duration::from_secs(3600))
            .connect(database_url)
            .await
            .map_err(|e| MantaError::Storage {
                context: format!("Failed to connect to database"),
                details: e.to_string(),
            })?;

        let store = Self {
            pool,
            cache: Arc::new(RwLock::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(1000).unwrap(),
            ))),
        };

        store.optimize().await?;
        store.init_schema().await?;

        info!("SQLite session store initialized");
        Ok(store)
    }

    /// Apply SQLite optimizations
    async fn optimize(&self) -> Result<()> {
        debug!("Applying database optimizations");

        // Enable WAL mode for better concurrency
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&self.pool)
            .await
            .map_err(|e| MantaError::Storage {
                context: format!("Failed to enable WAL mode"),
                details: e.to_string(),
            })?;

        // Enable foreign keys
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&self.pool)
            .await
            .map_err(|e| MantaError::Storage {
                context: format!("Failed to enable foreign keys"),
                details: e.to_string(),
            })?;

        // Set synchronous mode to NORMAL for better performance
        sqlx::query("PRAGMA synchronous = NORMAL")
            .execute(&self.pool)
            .await
            .map_err(|e| MantaError::Storage {
                context: format!("Failed to set synchronous mode"),
                details: e.to_string(),
            })?;

        Ok(())
    }

    /// Initialize database schema
    async fn init_schema(&self) -> Result<()> {
        debug!("Creating session storage schema");

        // Sessions table - stores session metadata and state
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                channel TEXT NOT NULL,
                channel_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_activity INTEGER NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1,
                state_json TEXT,
                message_count INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| MantaError::Storage {
            context: format!("Failed to create sessions table"),
            details: e.to_string(),
        })?;

        // Session messages table - stores conversation history
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS session_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                metadata TEXT,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| MantaError::Storage {
            context: format!("Failed to create messages table"),
            details: e.to_string(),
        })?;

        // Indexes for common queries
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_id)")
            .execute(&self.pool)
            .await
            .ok();

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_sessions_channel ON sessions(channel, channel_id)",
        )
        .execute(&self.pool)
        .await
        .ok();

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_activity ON sessions(last_activity)")
            .execute(&self.pool)
            .await
            .ok();

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_session ON session_messages(session_id, created_at)"
        )
        .execute(&self.pool)
        .await
        .ok();

        // ── Thread / Turn additions ───────────────────────────────────────────

        // Threads table: one row per named conversation branch.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS threads (
                id          TEXT    NOT NULL,
                session_id  TEXT    NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                label       TEXT    NOT NULL DEFAULT '',
                created_at  INTEGER NOT NULL,
                PRIMARY KEY (id, session_id)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to create threads table".to_string(),
            details: e.to_string(),
        })?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_threads_session ON threads(session_id)",
        )
        .execute(&self.pool)
        .await
        .ok();

        // Migrate existing session_messages rows: add thread_id, turn_index,
        // turn_state columns if they are not already present.
        // SQLite does not support ADD COLUMN IF NOT EXISTS, so we check
        // pragma_table_info first.
        let has_thread_id: bool = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pragma_table_info('session_messages') WHERE name='thread_id'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0) > 0;

        if !has_thread_id {
            sqlx::query("ALTER TABLE session_messages ADD COLUMN thread_id   TEXT")
                .execute(&self.pool)
                .await
                .map_err(|e| MantaError::Storage {
                    context: "Failed to add thread_id column".to_string(),
                    details: e.to_string(),
                })?;
            sqlx::query("ALTER TABLE session_messages ADD COLUMN turn_index  INTEGER")
                .execute(&self.pool)
                .await
                .map_err(|e| MantaError::Storage {
                    context: "Failed to add turn_index column".to_string(),
                    details: e.to_string(),
                })?;
            sqlx::query("ALTER TABLE session_messages ADD COLUMN turn_state  TEXT")
                .execute(&self.pool)
                .await
                .map_err(|e| MantaError::Storage {
                    context: "Failed to add turn_state column".to_string(),
                    details: e.to_string(),
                })?;
            debug!("Migrated session_messages: added thread_id, turn_index, turn_state columns");
        }

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_thread ON session_messages(session_id, thread_id, turn_index)"
        )
        .execute(&self.pool)
        .await
        .ok();

        info!("Session storage schema initialized");
        Ok(())
    }

    /// Save or update a session
    #[instrument(skip(self, metadata, state_json))]
    pub async fn save_session(
        &self,
        session_id: &str,
        metadata: &SessionMetadata,
        state_json: &str,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        let created_at = metadata.created_at.timestamp_millis();

        sqlx::query(
            r#"
            INSERT INTO sessions (id, agent_id, channel, channel_id, created_at, last_activity, is_active, state_json, message_count)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                agent_id = excluded.agent_id,
                channel = excluded.channel,
                channel_id = excluded.channel_id,
                last_activity = excluded.last_activity,
                is_active = excluded.is_active,
                state_json = excluded.state_json,
                message_count = excluded.message_count
            "#,
        )
        .bind(session_id)
        .bind(&metadata.agent_id)
        .bind(&metadata.channel)
        .bind(&metadata.channel_id)
        .bind(created_at)
        .bind(now)
        .bind(metadata.is_active)
        .bind(state_json)
        .bind(metadata.message_count as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| MantaError::Storage { context: format!("Failed to save session"), details: e.to_string() })?;

        // Update cache
        let mut cache = self.cache.write().await;
        cache.put(session_id.to_string(), Utc::now());

        debug!("Session saved: {}", session_id);
        Ok(())
    }

    /// Load a session by ID
    #[instrument(skip(self))]
    pub async fn load_session(&self, session_id: &str) -> Result<Option<PersistedSession>> {
        let row = sqlx::query(
            r#"
            SELECT id, agent_id, channel, channel_id, created_at, last_activity, is_active, state_json, message_count
            FROM sessions
            WHERE id = ?
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MantaError::Storage { context: format!("Failed to load session"), details: e.to_string() })?;

        match row {
            Some(row) => {
                let metadata = SessionMetadata {
                    session_id: row.get("id"),
                    agent_id: row.get("agent_id"),
                    channel: row.get("channel"),
                    channel_id: row.get("channel_id"),
                    created_at: DateTime::from_timestamp_millis(row.get::<i64, _>("created_at"))
                        .unwrap_or_else(Utc::now),
                    last_activity: DateTime::from_timestamp_millis(
                        row.get::<i64, _>("last_activity"),
                    )
                    .unwrap_or_else(Utc::now),
                    is_active: row.get::<i64, _>("is_active") != 0,
                    message_count: row.get::<i64, _>("message_count") as usize,
                };

                let session = PersistedSession {
                    id: row.get("id"),
                    metadata,
                    state_json: row.get("state_json"),
                    message_count: row.get::<i64, _>("message_count"),
                };

                // Update cache
                let mut cache = self.cache.write().await;
                cache.put(session_id.to_string(), Utc::now());

                debug!("Session loaded: {}", session_id);
                Ok(Some(session))
            }
            None => {
                debug!("Session not found: {}", session_id);
                Ok(None)
            }
        }
    }

    /// Find sessions by metadata
    #[instrument(skip(self))]
    pub async fn find_sessions(
        &self,
        agent_id: Option<&str>,
        channel: Option<&str>,
        channel_id: Option<&str>,
        active_only: bool,
    ) -> Result<Vec<SessionMetadata>> {
        let mut query = String::from(
            "SELECT id, agent_id, channel, channel_id, created_at, last_activity, is_active, message_count FROM sessions WHERE 1=1"
        );

        if agent_id.is_some() {
            query.push_str(" AND agent_id = ?");
        }
        if channel.is_some() {
            query.push_str(" AND channel = ?");
        }
        if channel_id.is_some() {
            query.push_str(" AND channel_id = ?");
        }
        if active_only {
            query.push_str(" AND is_active = 1");
        }

        query.push_str(" ORDER BY last_activity DESC");

        let mut sql_query =
            sqlx::query_as::<_, (String, String, String, String, i64, i64, i64, i64)>(&query);

        if let Some(agent) = agent_id {
            sql_query = sql_query.bind(agent);
        }
        if let Some(ch) = channel {
            sql_query = sql_query.bind(ch);
        }
        if let Some(ch_id) = channel_id {
            sql_query = sql_query.bind(ch_id);
        }

        let rows = sql_query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| MantaError::Storage {
                context: format!("Failed to find sessions"),
                details: e.to_string(),
            })?;

        let sessions: Vec<SessionMetadata> = rows
            .into_iter()
            .map(
                |(
                    id,
                    agent_id,
                    channel,
                    channel_id,
                    created_at,
                    last_activity,
                    is_active,
                    message_count,
                )| {
                    SessionMetadata {
                        session_id: id,
                        agent_id,
                        channel,
                        channel_id,
                        created_at: DateTime::from_timestamp_millis(created_at)
                            .unwrap_or_else(Utc::now),
                        last_activity: DateTime::from_timestamp_millis(last_activity)
                            .unwrap_or_else(Utc::now),
                        is_active: is_active != 0,
                        message_count: message_count as usize,
                    }
                },
            )
            .collect();

        debug!("Found {} sessions", sessions.len());
        Ok(sessions)
    }

    /// Append a message to session history
    #[instrument(skip(self, content))]
    pub async fn append_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        metadata_json: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();

        sqlx::query(
            r#"
            INSERT INTO session_messages (session_id, role, content, created_at, metadata)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(session_id)
        .bind(role)
        .bind(content)
        .bind(now)
        .bind(metadata_json)
        .execute(&self.pool)
        .await
        .map_err(|e| MantaError::Storage {
            context: format!("Failed to append message"),
            details: e.to_string(),
        })?;

        // Update message count
        sqlx::query(
            "UPDATE sessions SET message_count = message_count + 1, last_activity = ? WHERE id = ?",
        )
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();

        Ok(())
    }

    /// Get messages for a session
    #[instrument(skip(self))]
    pub async fn get_messages(
        &self,
        session_id: &str,
        limit: i64,
        before: Option<DateTime<Utc>>,
    ) -> Result<Vec<(String, String, DateTime<Utc>)>> {
        let before_ts = before.map(|dt| dt.timestamp_millis()).unwrap_or(i64::MAX);

        let rows = sqlx::query(
            r#"
            SELECT role, content, created_at
            FROM session_messages
            WHERE session_id = ? AND created_at < ?
            ORDER BY created_at DESC
            LIMIT ?
            "#,
        )
        .bind(session_id)
        .bind(before_ts)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MantaError::Storage {
            context: format!("Failed to get messages"),
            details: e.to_string(),
        })?;

        let messages: Vec<(String, String, DateTime<Utc>)> = rows
            .into_iter()
            .map(|row| {
                let role: String = row.get("role");
                let content: String = row.get("content");
                let ts: i64 = row.get("created_at");
                let dt = DateTime::from_timestamp_millis(ts).unwrap_or_else(Utc::now);
                (role, content, dt)
            })
            .collect();

        Ok(messages)
    }

    /// Set session active status
    pub async fn set_session_active(&self, session_id: &str, active: bool) -> Result<()> {
        sqlx::query("UPDATE sessions SET is_active = ?, last_activity = ? WHERE id = ?")
            .bind(if active { 1 } else { 0 })
            .bind(Utc::now().timestamp_millis())
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| MantaError::Storage {
                context: format!("Failed to update session status"),
                details: e.to_string(),
            })?;

        Ok(())
    }

    /// Delete a session and all its messages
    #[instrument(skip(self))]
    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| MantaError::Storage {
                context: format!("Failed to delete session"),
                details: e.to_string(),
            })?;

        // Cache cleanup
        let mut cache = self.cache.write().await;
        cache.pop(session_id);

        info!("Session deleted: {}", session_id);
        Ok(())
    }

    /// Cleanup old inactive sessions
    #[instrument(skip(self))]
    pub async fn cleanup_old_sessions(&self, older_than: Duration) -> Result<usize> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(older_than).unwrap_or(chrono::Duration::days(30));

        let result = sqlx::query("DELETE FROM sessions WHERE is_active = 0 AND last_activity < ?")
            .bind(cutoff.timestamp_millis())
            .execute(&self.pool)
            .await
            .map_err(|e| MantaError::Storage {
                context: format!("Failed to cleanup sessions"),
                details: e.to_string(),
            })?;

        let deleted = result.rows_affected() as usize;
        info!("Cleaned up {} old sessions", deleted);
        Ok(deleted)
    }

    // ── Thread / Turn methods ─────────────────────────────────────────────────

    /// Upsert a thread record.  Call when a Thread is first created for a session.
    pub async fn save_thread(
        &self,
        session_id: &str,
        thread_id: &str,
        label: &str,
        created_at_ms: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO threads (id, session_id, label, created_at)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(id, session_id) DO UPDATE SET label = excluded.label
            "#,
        )
        .bind(thread_id)
        .bind(session_id)
        .bind(label)
        .bind(created_at_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to save thread".to_string(),
            details: e.to_string(),
        })?;

        debug!("Thread saved: {} / {}", session_id, thread_id);
        Ok(())
    }

    /// Append a completed turn as two rows (user + assistant) tagged with
    /// thread and turn metadata.
    pub async fn append_turn(
        &self,
        session_id: &str,
        thread_id: &str,
        turn_index: i64,
        user_msg: &str,
        assistant_msg: &str,
        state: &str,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();

        // Insert user message row.
        sqlx::query(
            r#"
            INSERT INTO session_messages (session_id, role, content, created_at, thread_id, turn_index, turn_state)
            VALUES (?, 'user', ?, ?, ?, ?, ?)
            "#,
        )
        .bind(session_id)
        .bind(user_msg)
        .bind(now)
        .bind(thread_id)
        .bind(turn_index)
        .bind(state)
        .execute(&self.pool)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to insert turn user message".to_string(),
            details: e.to_string(),
        })?;

        // Insert assistant message row.
        sqlx::query(
            r#"
            INSERT INTO session_messages (session_id, role, content, created_at, thread_id, turn_index, turn_state)
            VALUES (?, 'assistant', ?, ?, ?, ?, ?)
            "#,
        )
        .bind(session_id)
        .bind(assistant_msg)
        .bind(now + 1) // ensure deterministic ordering
        .bind(thread_id)
        .bind(turn_index)
        .bind(state)
        .execute(&self.pool)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to insert turn assistant message".to_string(),
            details: e.to_string(),
        })?;

        // Keep session message_count in sync.
        sqlx::query(
            "UPDATE sessions SET message_count = message_count + 2, last_activity = ? WHERE id = ?",
        )
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();

        debug!("Turn appended: {}/{} index={}", session_id, thread_id, turn_index);
        Ok(())
    }

    /// Load all threads for a session together with their turns.
    ///
    /// Returns `Vec<(thread_id, label, created_at_ms, Vec<(turn_index, user_msg, asst_msg, state)>)>`.
    pub async fn load_threads_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<(String, String, i64, Vec<(i64, String, String, String)>)>> {
        // Load thread rows.
        let thread_rows = sqlx::query(
            "SELECT id, label, created_at FROM threads WHERE session_id = ? ORDER BY created_at",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to load threads".to_string(),
            details: e.to_string(),
        })?;

        let mut result = Vec::new();

        for trow in thread_rows {
            let tid: String = trow.get("id");
            let label: String = trow.get("label");
            let created_at: i64 = trow.get("created_at");

            // Load user-half of each turn (role='user') ordered by turn_index.
            // We join with the assistant row by matching (session_id, thread_id, turn_index).
            let turn_rows = sqlx::query(
                r#"
                SELECT u.turn_index,
                       u.content      AS user_msg,
                       COALESCE(a.content, '') AS asst_msg,
                       COALESCE(u.turn_state, 'complete') AS state
                FROM session_messages u
                LEFT JOIN session_messages a
                    ON  a.session_id = u.session_id
                    AND a.thread_id  = u.thread_id
                    AND a.turn_index = u.turn_index
                    AND a.role       = 'assistant'
                WHERE u.session_id = ?
                  AND u.thread_id  = ?
                  AND u.role       = 'user'
                ORDER BY u.turn_index
                "#,
            )
            .bind(session_id)
            .bind(&tid)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| MantaError::Storage {
                context: "Failed to load turns".to_string(),
                details: e.to_string(),
            })?;

            let turns: Vec<(i64, String, String, String)> = turn_rows
                .into_iter()
                .map(|r| {
                    let idx: i64 = r.get("turn_index");
                    let user: String = r.get("user_msg");
                    let asst: String = r.get("asst_msg");
                    let st: String = r.get("state");
                    (idx, user, asst, st)
                })
                .collect();

            result.push((tid, label, created_at, turns));
        }

        debug!(
            "Loaded {} threads for session {}",
            result.len(),
            session_id
        );
        Ok(result)
    }

    /// Hard-delete all rows for a specific turn (used by undo persistence).
    pub async fn delete_turn(
        &self,
        session_id: &str,
        thread_id: &str,
        turn_index: i64,
    ) -> Result<()> {
        let affected = sqlx::query(
            "DELETE FROM session_messages WHERE session_id = ? AND thread_id = ? AND turn_index = ?",
        )
        .bind(session_id)
        .bind(thread_id)
        .bind(turn_index)
        .execute(&self.pool)
        .await
        .map_err(|e| MantaError::Storage {
            context: "Failed to delete turn".to_string(),
            details: e.to_string(),
        })?
        .rows_affected();

        // Adjust session message_count (deleted rows are usually 2).
        sqlx::query(
            "UPDATE sessions SET message_count = MAX(0, message_count - ?) WHERE id = ?",
        )
        .bind(affected as i64)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();

        debug!(
            "Deleted turn {}/{}/{}: {} rows",
            session_id, thread_id, turn_index, affected
        );
        Ok(())
    }

    /// Get session statistics
    pub async fn get_stats(&self) -> Result<SessionStats> {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);

        let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE is_active = 1")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);

        let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session_messages")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);

        Ok(SessionStats {
            total_sessions: total,
            active_sessions: active,
            total_messages: messages,
        })
    }
}

/// Session statistics
#[derive(Debug, Clone)]
pub struct SessionStats {
    pub total_sessions: i64,
    pub active_sessions: i64,
    pub total_messages: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_store() -> SessionStore {
        // Use in-memory SQLite for tests
        SessionStore::new(":memory:")
            .await
            .expect("Failed to create test store")
    }

    #[tokio::test]
    async fn test_save_and_load_session() {
        let store = create_test_store().await;

        let metadata = SessionMetadata::new("test-session", "main", "discord", "user123");

        // Save session
        store
            .save_session("test-session", &metadata, r#"{"key": "value"}"#)
            .await
            .expect("Failed to save session");

        // Load session
        let loaded = store
            .load_session("test-session")
            .await
            .expect("Failed to load session")
            .expect("Session not found");

        assert_eq!(loaded.id, "test-session");
        assert_eq!(loaded.metadata.agent_id, "main");
        assert_eq!(loaded.metadata.channel, "discord");
    }

    #[tokio::test]
    async fn test_find_sessions() {
        let store = create_test_store().await;

        // Create multiple sessions
        for i in 0..3 {
            let metadata = SessionMetadata::new(
                format!("session-{}", i),
                if i == 0 { "main" } else { "coder" },
                "discord",
                format!("user{}", i),
            );
            store
                .save_session(&format!("session-{}", i), &metadata, "{}")
                .await
                .expect("Failed to save session");
        }

        // Find by agent
        let main_sessions = store
            .find_sessions(Some("main"), None, None, false)
            .await
            .expect("Failed to find sessions");
        assert_eq!(main_sessions.len(), 1);
        assert_eq!(main_sessions[0].agent_id, "main");

        // Find by channel
        let discord_sessions = store
            .find_sessions(None, Some("discord"), None, false)
            .await
            .expect("Failed to find sessions");
        assert_eq!(discord_sessions.len(), 3);
    }

    #[tokio::test]
    async fn test_messages() {
        let store = create_test_store().await;

        // Create session
        let metadata = SessionMetadata::new("msg-test", "main", "cli", "local");
        store
            .save_session("msg-test", &metadata, "{}")
            .await
            .expect("Failed to save session");

        // Append messages
        store
            .append_message("msg-test", "user", "Hello", None)
            .await
            .expect("Failed to append message");

        store
            .append_message("msg-test", "assistant", "Hi there!", None)
            .await
            .expect("Failed to append message");

        // Get messages
        let messages = store
            .get_messages("msg-test", 10, None)
            .await
            .expect("Failed to get messages");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].0, "assistant"); // Most recent first
        assert_eq!(messages[1].0, "user");
    }
}
