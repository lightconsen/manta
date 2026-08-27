//! Turn feedback store.
//!
//! Persists per-turn Like/Dislike votes submitted via the WS `feedback.vote`
//! method. Each row is keyed by the stable `turn_id` emitted in the
//! `chat.final` event, so a vote survives page reloads and can be updated
//! in place (ON CONFLICT DO UPDATE) when the user re-votes or toggles.

use std::time::Duration;

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Pool, Sqlite};
use tracing::{info, instrument};

use crate::error::{Result, SyscityError};

/// Vote direction for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackVoteKind {
    /// Positive feedback (Like / 👍)
    Up,
    /// Negative feedback (Dislike / 👎)
    Down,
}

impl FeedbackVoteKind {
    /// Database representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            FeedbackVoteKind::Up => "up",
            FeedbackVoteKind::Down => "down",
        }
    }

    /// Parse from the database representation; `None` for unknown values.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "up" => Some(FeedbackVoteKind::Up),
            "down" => Some(FeedbackVoteKind::Down),
            _ => None,
        }
    }
}

/// A persisted turn feedback vote.
#[derive(Debug, Clone)]
pub struct FeedbackVote {
    pub turn_id: String,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub vote: FeedbackVoteKind,
    pub comment: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Parameters for [`FeedbackStore::upsert_vote`].
#[derive(Debug, Clone)]
pub struct UpsertVoteParams {
    pub turn_id: String,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub vote: FeedbackVoteKind,
    pub comment: Option<String>,
}

/// SQLite-backed store for per-turn Like/Dislike feedback.
#[derive(Debug, Clone)]
pub struct FeedbackStore {
    pool: Pool<Sqlite>,
}

impl FeedbackStore {
    /// Create a new feedback store from a database URL.
    #[instrument(skip(database_url))]
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(30))
            .connect(database_url)
            .await
            .map_err(|e| SyscityError::Storage {
                context: "Failed to connect to feedback database".to_string(),
                details: e.to_string(),
            })?;
        Self::from_pool(pool).await
    }

    /// Create a feedback store from an existing connection pool.
    ///
    /// Must be handed the same pool shared by the other stores (not a fresh
    /// `:memory:` connection) so all tables live in one database.
    pub async fn from_pool(pool: Pool<Sqlite>) -> Result<Self> {
        let store = Self { pool };
        store.optimize().await?;
        store.init_schema().await?;
        info!("Feedback store initialized");
        Ok(store)
    }

    /// Apply SQLite optimizations.
    async fn optimize(&self) -> Result<()> {
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&self.pool)
            .await
            .map_err(|e| SyscityError::Storage {
                context: "Failed to enable WAL mode".to_string(),
                details: e.to_string(),
            })?;
        sqlx::query("PRAGMA synchronous = NORMAL")
            .execute(&self.pool)
            .await
            .map_err(|e| SyscityError::Storage {
                context: "Failed to set synchronous mode".to_string(),
                details: e.to_string(),
            })?;
        Ok(())
    }

    /// Initialize the `turn_feedback` table.
    async fn init_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS turn_feedback (
                turn_id     TEXT PRIMARY KEY,
                session_id  TEXT,
                agent_id    TEXT,
                vote        TEXT NOT NULL CHECK (vote IN ('up','down')),
                comment     TEXT,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to create turn_feedback table".to_string(),
            details: e.to_string(),
        })?;
        Ok(())
    }

    /// Insert a vote, or update the existing one for the same `turn_id`.
    pub async fn upsert_vote(&self, params: &UpsertVoteParams) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            r#"
            INSERT INTO turn_feedback (turn_id, session_id, agent_id, vote, comment, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
            ON CONFLICT (turn_id) DO UPDATE SET
                vote = excluded.vote,
                comment = excluded.comment,
                session_id = excluded.session_id,
                agent_id = excluded.agent_id,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&params.turn_id)
        .bind(params.session_id.as_deref())
        .bind(params.agent_id.as_deref())
        .bind(params.vote.as_str())
        .bind(params.comment.as_deref())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to upsert turn feedback".to_string(),
            details: e.to_string(),
        })?;
        Ok(())
    }

    /// Fetch the current vote for a turn, if any.
    pub async fn get_vote(&self, turn_id: &str) -> Result<Option<FeedbackVote>> {
        let row = sqlx::query(
            r#"
            SELECT turn_id, session_id, agent_id, vote, comment, created_at, updated_at
            FROM turn_feedback WHERE turn_id = ?1
            "#,
        )
        .bind(turn_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to fetch turn feedback".to_string(),
            details: e.to_string(),
        })?;

        row.map(|r| feedback_vote_from_row(&r)).transpose()
    }

    /// List votes filtered by direction, newest first.
    pub async fn list_votes_by(
        &self,
        vote: FeedbackVoteKind,
        since_ms: i64,
        limit: u32,
    ) -> Result<Vec<FeedbackVote>> {
        let rows = sqlx::query(
            r#"
            SELECT turn_id, session_id, agent_id, vote, comment, created_at, updated_at
            FROM turn_feedback
            WHERE vote = ?1 AND created_at >= ?2
            ORDER BY created_at DESC, rowid DESC
            LIMIT ?3
            "#,
        )
        .bind(vote.as_str())
        .bind(since_ms)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to list turn feedback".to_string(),
            details: e.to_string(),
        })?;

        rows.iter().map(feedback_vote_from_row).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_store() -> FeedbackStore {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        FeedbackStore::from_pool(pool).await.unwrap()
    }

    #[tokio::test]
    async fn upsert_then_get_roundtrips() {
        let store = test_store().await;
        store
            .upsert_vote(&UpsertVoteParams {
                turn_id: "t1".into(),
                session_id: Some("s1".into()),
                agent_id: Some("a1".into()),
                vote: FeedbackVoteKind::Up,
                comment: Some("nice".into()),
            })
            .await
            .unwrap();

        let vote = store.get_vote("t1").await.unwrap().unwrap();
        assert_eq!(vote.turn_id, "t1");
        assert_eq!(vote.vote, FeedbackVoteKind::Up);
        assert_eq!(vote.comment.as_deref(), Some("nice"));
    }

    #[tokio::test]
    async fn revote_updates_in_place_and_tracks_created_at() {
        let store = test_store().await;
        let created_0 = chrono::Utc::now().timestamp_millis();
        store
            .upsert_vote(&UpsertVoteParams {
                turn_id: "t1".into(),
                session_id: None,
                agent_id: None,
                vote: FeedbackVoteKind::Down,
                comment: None,
            })
            .await
            .unwrap();
        let first = store.get_vote("t1").await.unwrap().unwrap();
        assert!(first.created_at >= created_0);

        // Toggle to up: same row, updated vote + timestamp.
        std::thread::sleep(std::time::Duration::from_millis(5));
        store
            .upsert_vote(&UpsertVoteParams {
                turn_id: "t1".into(),
                session_id: None,
                agent_id: None,
                vote: FeedbackVoteKind::Up,
                comment: None,
            })
            .await
            .unwrap();

        let second = store.get_vote("t1").await.unwrap().unwrap();
        assert_eq!(second.vote, FeedbackVoteKind::Up);
        assert_eq!(second.created_at, first.created_at); // created_at preserved
        assert!(second.updated_at >= first.updated_at);
    }

    #[tokio::test]
    async fn list_votes_by_filters_and_orders() {
        let store = test_store().await;
        store
            .upsert_vote(&UpsertVoteParams {
                turn_id: "t1".into(),
                session_id: None,
                agent_id: None,
                vote: FeedbackVoteKind::Down,
                comment: None,
            })
            .await
            .unwrap();
        store
            .upsert_vote(&UpsertVoteParams {
                turn_id: "t2".into(),
                session_id: None,
                agent_id: None,
                vote: FeedbackVoteKind::Up,
                comment: None,
            })
            .await
            .unwrap();
        store
            .upsert_vote(&UpsertVoteParams {
                turn_id: "t3".into(),
                session_id: None,
                agent_id: None,
                vote: FeedbackVoteKind::Down,
                comment: None,
            })
            .await
            .unwrap();

        let downs = store
            .list_votes_by(FeedbackVoteKind::Down, 0, 10)
            .await
            .unwrap();
        assert_eq!(downs.len(), 2);
        // Newest first.
        assert_eq!(downs[0].turn_id, "t3");
        assert_eq!(downs[1].turn_id, "t1");
    }
}

fn feedback_vote_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> std::result::Result<FeedbackVote, SyscityError> {
    use sqlx::Row;
    let turn_id: String = row.get("turn_id");
    let session_id: Option<String> = row.get("session_id");
    let agent_id: Option<String> = row.get("agent_id");
    let vote: String = row.get("vote");
    let comment: Option<String> = row.get("comment");
    let created_at: i64 = row.get("created_at");
    let updated_at: i64 = row.get("updated_at");
    let vote = FeedbackVoteKind::from_str(&vote).ok_or_else(|| SyscityError::Storage {
        context: "Invalid vote value in turn_feedback".to_string(),
        details: format!("unexpected vote: {vote}"),
    })?;
    Ok(FeedbackVote {
        turn_id,
        session_id,
        agent_id,
        vote,
        comment,
        created_at,
        updated_at,
    })
}
