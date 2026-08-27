//! Pending badcase collection store.
//!
//! Badcases are auto-collected online from two sources before they enter the
//! RCA / regression pipeline:
//!
//! - `online:risk`  — deterministic `RiskSignalChecker` findings on a turn.
//! - `human:dislike` — a user's 👎 vote on a turn (`feedback.vote`).
//!
//! Rows are deduplicated by `sha256(input || '|' || response)`, so the same
//! failure is only queued once regardless of how many signals or votes fire on
//! it. An operator (or the auto pipeline) later confirms a row, which commits
//! it as a `BadcaseRecord` + YAML via [`recycle::write_badcase_yaml`], or
//! dismisses it.

use std::path::PathBuf;

use sha2::{Digest, Sha256};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Pool, Sqlite};
use tracing::{info, instrument};

use crate::error::{Result, SyscityError};
use crate::eval::loader::default_evals_dir;

use super::dataset::{EvalTask, EvalTaskSource};
use super::rca::BadcaseEntry;
use super::recycle::{write_badcase_yaml, BadcaseFixStatus, BadcaseRecord};

/// Where a pending badcase came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingSource {
    /// Deterministic online risk-signal scan of a completed turn.
    OnlineRisk,
    /// A human Like/Dislike vote (👎) on a turn.
    HumanDislike,
}

impl PendingSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            PendingSource::OnlineRisk => "online:risk",
            PendingSource::HumanDislike => "human:dislike",
        }
    }
}

/// Lifecycle of a pending badcase row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingStatus {
    /// Collected, awaiting confirmation.
    Pending,
    /// Reviewed and accepted as a real badcase (pre-commit).
    Confirmed,
    /// Committed into a `BadcaseRecord` + YAML.
    Converted,
    /// Reviewed and rejected.
    Dismissed,
}

impl PendingStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PendingStatus::Pending => "pending",
            PendingStatus::Confirmed => "confirmed",
            PendingStatus::Converted => "converted",
            PendingStatus::Dismissed => "dismissed",
        }
    }
}

/// A row in the pending badcase pool.
#[derive(Debug, Clone)]
pub struct PendingBadcase {
    pub id: String,
    pub source: PendingSource,
    pub turn_id: Option<String>,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub input: String,
    pub response: String,
    pub risk_signals: Vec<String>,
    pub status: PendingStatus,
    pub created_at: i64,
}

/// Parameters for [`PendingBadcaseStore::insert_pending`].
#[derive(Debug, Clone)]
pub struct InsertPendingParams {
    pub source: PendingSource,
    pub turn_id: Option<String>,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub input: String,
    pub response: String,
    pub risk_signals: Vec<String>,
}

/// Compute the dedup hash for an input/response pair.
pub fn dedup_hash(input: &str, response: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hasher.update(b"|");
    hasher.update(response.as_bytes());
    hex::encode(hasher.finalize())
}

/// SQLite-backed store for the pending badcase pool.
#[derive(Debug, Clone)]
pub struct PendingBadcaseStore {
    pool: Pool<Sqlite>,
}

impl PendingBadcaseStore {
    /// Create a new store from a database URL.
    #[instrument(skip(database_url))]
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .acquire_timeout(std::time::Duration::from_secs(30))
            .connect(database_url)
            .await
            .map_err(|e| SyscityError::Storage {
                context: "Failed to connect to pending badcase database".to_string(),
                details: e.to_string(),
            })?;
        Self::from_pool(pool).await
    }

    /// Create a store from an existing connection pool (shared with the other
    /// stores so all tables live in one database).
    pub async fn from_pool(pool: Pool<Sqlite>) -> Result<Self> {
        let store = Self { pool };
        store.init_schema().await?;
        info!("Pending badcase store initialized");
        Ok(store)
    }

    async fn init_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS pending_badcases (
                id            TEXT PRIMARY KEY,
                source        TEXT NOT NULL CHECK (source IN ('online:risk','human:dislike')),
                turn_id       TEXT,
                session_id    TEXT,
                agent_id      TEXT,
                input         TEXT NOT NULL,
                response      TEXT NOT NULL,
                risk_signals  TEXT NOT NULL DEFAULT '[]',
                dedup_hash    TEXT NOT NULL UNIQUE,
                status        TEXT NOT NULL CHECK (status IN ('pending','confirmed','converted','dismissed')),
                created_at    INTEGER NOT NULL,
                updated_at    INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to create pending_badcases table".to_string(),
            details: e.to_string(),
        })?;
        Ok(())
    }

    /// Insert a pending badcase unless the same input/response already exists.
    ///
    /// Returns `Ok(true)` when inserted, `Ok(false)` when deduplicated.
    pub async fn insert_pending(&self, params: &InsertPendingParams) -> Result<bool> {
        let now = chrono::Utc::now().timestamp_millis();
        let id = uuid::Uuid::new_v4().to_string();
        let hash = dedup_hash(&params.input, &params.response);
        let signals_json =
            serde_json::to_string(&params.risk_signals).map_err(|e| SyscityError::Storage {
                context: "Failed to serialize risk signals".to_string(),
                details: e.to_string(),
            })?;

        let result = sqlx::query(
            r#"
            INSERT INTO pending_badcases
                (id, source, turn_id, session_id, agent_id, input, response, risk_signals, dedup_hash, status, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', ?10, ?10)
            ON CONFLICT (dedup_hash) DO NOTHING
            "#,
        )
        .bind(&id)
        .bind(params.source.as_str())
        .bind(params.turn_id.as_deref())
        .bind(params.session_id.as_deref())
        .bind(params.agent_id.as_deref())
        .bind(&params.input)
        .bind(&params.response)
        .bind(&signals_json)
        .bind(&hash)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to insert pending badcase".to_string(),
            details: e.to_string(),
        })?;

        Ok(result.rows_affected() > 0)
    }

    /// List pending rows by status, oldest first.
    pub async fn list_pending(
        &self,
        status: PendingStatus,
        limit: u32,
    ) -> Result<Vec<PendingBadcase>> {
        let rows = sqlx::query(
            r#"
            SELECT id, source, turn_id, session_id, agent_id, input, response, risk_signals, status, created_at
            FROM pending_badcases
            WHERE status = ?1
            ORDER BY created_at ASC
            LIMIT ?2
            "#,
        )
        .bind(status.as_str())
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to list pending badcases".to_string(),
            details: e.to_string(),
        })?;

        rows.iter()
            .map(|row| {
                use sqlx::Row;
                let risk_signals_json: String = row.get("risk_signals");
                let risk_signals: Vec<String> =
                    serde_json::from_str(&risk_signals_json).unwrap_or_default();
                Ok(PendingBadcase {
                    id: row.get("id"),
                    source: parse_source(row.get("source"))?,
                    turn_id: row.get("turn_id"),
                    session_id: row.get("session_id"),
                    agent_id: row.get("agent_id"),
                    input: row.get("input"),
                    response: row.get("response"),
                    risk_signals,
                    status: parse_status(row.get("status"))?,
                    created_at: row.get("created_at"),
                })
            })
            .collect()
    }

    /// Mark a pending row confirmed.
    pub async fn confirm(&self, id: &str) -> Result<()> {
        self.set_status(id, PendingStatus::Confirmed).await
    }

    /// Mark a pending row dismissed (false positive / duplicate).
    pub async fn dismiss(&self, id: &str) -> Result<()> {
        self.set_status(id, PendingStatus::Dismissed).await
    }

    /// Confirm a pending badcase and commit it into the badcase regression
    /// suite as a `BadcaseRecord` + YAML file.
    ///
    /// Returns the written YAML path. Idempotent: a row already `converted`
    /// returns the existing path without rewriting.
    pub async fn confirm_and_commit(&self, id: &str) -> Result<PathBuf> {
        let row = sqlx::query(
            r#"
            SELECT id, source, turn_id, session_id, agent_id, input, response, risk_signals, status, created_at
            FROM pending_badcases WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to fetch pending badcase".to_string(),
            details: e.to_string(),
        })?;

        let Some(row) = row else {
            return Err(SyscityError::Storage {
                context: "Pending badcase not found".to_string(),
                details: format!("no row with id {id}"),
            });
        };

        use sqlx::Row;
        let status: String = row.get("status");
        let source = parse_source(row.get("source"))?;
        let input: String = row.get("input");
        let response: String = row.get("response");
        let turn_id: Option<String> = row.get("turn_id");
        let risk_signals_json: String = row.get("risk_signals");
        let risk_signals: Vec<String> =
            serde_json::from_str(&risk_signals_json).unwrap_or_default();

        // A row already committed can be confirmed again without a rewrite.
        if status == PendingStatus::Converted.as_str() {
            let existing = self.commit_path(id);
            if existing.exists() {
                return Ok(existing);
            }
        }

        let output_dir = self.commit_dir();
        let task_id = format!("badcase-{}", &id[..8.min(id.len())]);
        let record = BadcaseRecord {
            id: format!("{task_id}_{}", &id[..8]),
            task_id: task_id.clone(),
            input: input.clone(),
            description: source_description(source),
            failure_reason: if risk_signals.is_empty() {
                source_description(source)
            } else {
                risk_signals.join("; ")
            },
            response: response.clone(),
            rca_performed: false,
            rca_result: None,
            collected_at: std::time::SystemTime::now(),
            fix_status: BadcaseFixStatus::Unconfirmed,
            entry: badcase_entry_for(source),
        };

        let original = EvalTask {
            id: task_id,
            description: record.description.clone(),
            input,
            source: EvalTaskSource::BadcaseRecycle,
            failure_reason: Some(record.failure_reason.clone()),
            ..Default::default()
        };

        let path = write_badcase_yaml(&record, &original, &output_dir)?;
        self.set_status(id, PendingStatus::Converted).await?;

        if let Some(tid) = turn_id {
            info!("Committed pending badcase {id} from turn {tid} to {:?}", path);
        }
        Ok(path)
    }

    async fn set_status(&self, id: &str, status: PendingStatus) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("UPDATE pending_badcases SET status = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(status.as_str())
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| SyscityError::Storage {
                context: "Failed to update pending badcase status".to_string(),
                details: e.to_string(),
            })?;
        Ok(())
    }

    /// Directory where committed badcase YAML files are written.
    fn commit_dir(&self) -> PathBuf {
        default_evals_dir().join("badcases")
    }

    /// The expected YAML path for a committed badcase (for idempotent reuse).
    fn commit_path(&self, id: &str) -> PathBuf {
        self.commit_dir()
            .join(format!("{}.yaml", &id[..8.min(id.len())]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_store() -> PendingBadcaseStore {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        PendingBadcaseStore::from_pool(pool).await.unwrap()
    }

    fn params(source: PendingSource, input: &str, response: &str) -> InsertPendingParams {
        InsertPendingParams {
            source,
            turn_id: Some("t1".into()),
            session_id: None,
            agent_id: None,
            input: input.to_string(),
            response: response.to_string(),
            risk_signals: vec!["PII detected".to_string()],
        }
    }

    #[tokio::test]
    async fn insert_is_deduplicated_on_input_response() {
        let store = test_store().await;
        let first = store
            .insert_pending(&params(PendingSource::OnlineRisk, "hello", "world"))
            .await
            .unwrap();
        assert!(first, "first insert should succeed");

        let second = store
            .insert_pending(&params(PendingSource::OnlineRisk, "hello", "world"))
            .await
            .unwrap();
        assert!(!second, "duplicate input|response should be rejected");

        // Same input, different response is a distinct badcase.
        let third = store
            .insert_pending(&params(PendingSource::OnlineRisk, "hello", "world2"))
            .await
            .unwrap();
        assert!(third);
    }

    #[tokio::test]
    async fn list_filters_by_status() {
        let store = test_store().await;
        store
            .insert_pending(&params(PendingSource::HumanDislike, "u1", "r1"))
            .await
            .unwrap();
        store
            .insert_pending(&params(PendingSource::OnlineRisk, "u2", "r2"))
            .await
            .unwrap();

        let pending = store
            .list_pending(PendingStatus::Pending, 10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 2);
        // Oldest-first: the HumanDislike row was inserted first.
        assert_eq!(pending[0].source, PendingSource::HumanDislike);
        assert_eq!(pending[1].source, PendingSource::OnlineRisk);
        assert_eq!(pending[0].risk_signals, vec!["PII detected"]);

        let converted = store
            .list_pending(PendingStatus::Converted, 10)
            .await
            .unwrap();
        assert!(converted.is_empty());
    }

    #[tokio::test]
    async fn dismiss_and_confirm_flip_status() {
        let store = test_store().await;
        store
            .insert_pending(&params(PendingSource::HumanDislike, "u1", "r1"))
            .await
            .unwrap();
        let id = store
            .list_pending(PendingStatus::Pending, 10)
            .await
            .unwrap()[0]
            .id
            .clone();

        store.dismiss(&id).await.unwrap();
        assert!(store
            .list_pending(PendingStatus::Pending, 10)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .list_pending(PendingStatus::Dismissed, 10)
                .await
                .unwrap()
                .len(),
            1
        );

        store.confirm(&id).await.unwrap();
        assert_eq!(
            store
                .list_pending(PendingStatus::Confirmed, 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn confirm_and_commit_writes_yaml() {
        let store = test_store().await;
        store
            .insert_pending(&params(PendingSource::OnlineRisk, "u1", "r1"))
            .await
            .unwrap();
        let id = store
            .list_pending(PendingStatus::Pending, 10)
            .await
            .unwrap()[0]
            .id
            .clone();

        let path = store.confirm_and_commit(&id).await.unwrap();
        assert!(path.exists(), "committed badcase YAML should exist");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("tasks"));
        assert!(content.contains("u1"));

        // Row is now converted; committing again is idempotent.
        let path2 = store.confirm_and_commit(&id).await.unwrap();
        assert_eq!(path, path2);
        assert_eq!(
            store
                .list_pending(PendingStatus::Converted, 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}

fn parse_source(s: String) -> Result<PendingSource> {
    match s.as_str() {
        "online:risk" => Ok(PendingSource::OnlineRisk),
        "human:dislike" => Ok(PendingSource::HumanDislike),
        other => Err(SyscityError::Storage {
            context: "Invalid source in pending_badcases".to_string(),
            details: format!("unexpected source: {other}"),
        }),
    }
}

fn parse_status(s: String) -> Result<PendingStatus> {
    match s.as_str() {
        "pending" => Ok(PendingStatus::Pending),
        "confirmed" => Ok(PendingStatus::Confirmed),
        "converted" => Ok(PendingStatus::Converted),
        "dismissed" => Ok(PendingStatus::Dismissed),
        other => Err(SyscityError::Storage {
            context: "Invalid status in pending_badcases".to_string(),
            details: format!("unexpected status: {other}"),
        }),
    }
}

fn source_description(source: PendingSource) -> String {
    match source {
        PendingSource::OnlineRisk => {
            "Auto-collected online: deterministic risk signal detected on a turn.".to_string()
        }
        PendingSource::HumanDislike => {
            "Auto-collected online: user disliked (👎) this turn.".to_string()
        }
    }
}

fn badcase_entry_for(source: PendingSource) -> BadcaseEntry {
    match source {
        PendingSource::OnlineRisk => BadcaseEntry::OnlineRisk,
        PendingSource::HumanDislike => BadcaseEntry::HumanVote,
    }
}
