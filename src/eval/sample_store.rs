//! Production-turn online sampling store.
//!
//! Persists a sampled subset of completed production turns (`turn_samples`) so
//! the scoring / compression-gate / feedback-aggregation / shadow-replay
//! pipelines can read a stable, queryable snapshot of live traffic. Sampling
//! is opt-in: it stays disabled unless `eval.sampling.enabled` is set, so
//! existing deployments never write rows they did not ask for.

use std::time::Duration;

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Pool, Sqlite};
use tracing::{info, instrument};

use crate::error::{Result, SyscityError};

/// Verdict for a sampled production turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleVerdict {
    /// No risk signals fired on the turn.
    Pass,
    /// At least one risk signal fired on the turn.
    Flag,
    /// The turn errored during processing.
    Error,
}

impl SampleVerdict {
    /// Database representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            SampleVerdict::Pass => "pass",
            SampleVerdict::Flag => "flag",
            SampleVerdict::Error => "error",
        }
    }

    /// Parse from the database representation; `None` for unknown values.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pass" => Some(SampleVerdict::Pass),
            "flag" => Some(SampleVerdict::Flag),
            "error" => Some(SampleVerdict::Error),
            _ => None,
        }
    }
}

/// A row in the production turn sampling pool.
#[derive(Debug, Clone)]
pub struct TurnSample {
    pub turn_id: String,
    pub session_id: Option<String>,
    pub agent_id: String,
    pub conversation_id: String,
    pub input: String,
    pub response: String,
    pub model: String,
    pub cache_hit: bool,
    pub total_tokens: u64,
    pub latency_ms: u64,
    pub verdict: SampleVerdict,
    pub risk_signals: Vec<String>,
    pub created_at: i64,
}

/// Parameters for [`TurnSampleStore::insert_sample`] (`created_at` is filled
/// by the store).
#[derive(Debug, Clone)]
pub struct InsertSampleParams {
    pub turn_id: String,
    pub session_id: Option<String>,
    pub agent_id: String,
    pub conversation_id: String,
    pub input: String,
    pub response: String,
    pub model: String,
    pub cache_hit: bool,
    pub total_tokens: u64,
    pub latency_ms: u64,
    pub verdict: SampleVerdict,
    pub risk_signals: Vec<String>,
}

/// SQLite-backed store for sampled production turns.
#[derive(Debug, Clone)]
pub struct TurnSampleStore {
    pool: Pool<Sqlite>,
}

impl TurnSampleStore {
    /// Create a new store from a database URL.
    #[instrument(skip(database_url))]
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(30))
            .connect(database_url)
            .await
            .map_err(|e| SyscityError::Storage {
                context: "Failed to connect to turn sample database".to_string(),
                details: e.to_string(),
            })?;
        Self::from_pool(pool).await
    }

    /// Create a store from an existing connection pool.
    ///
    /// Must be handed the same pool shared by the other stores (not a fresh
    /// `:memory:` connection) so all tables live in one database.
    pub async fn from_pool(pool: Pool<Sqlite>) -> Result<Self> {
        let store = Self { pool };
        store.optimize().await?;
        store.init_schema().await?;
        info!("Turn sample store initialized");
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

    /// Initialize the `turn_samples` table.
    async fn init_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS turn_samples (
                turn_id TEXT PRIMARY KEY, session_id TEXT, agent_id TEXT, conversation_id TEXT,
                input TEXT, response TEXT, model TEXT, cache_hit INTEGER,
                total_tokens INTEGER, latency_ms INTEGER,
                verdict TEXT NOT NULL CHECK (verdict IN ('pass','flag','error')) DEFAULT 'pass',
                risk_signals TEXT NOT NULL DEFAULT '[]', created_at INTEGER NOT NULL)
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to create turn_samples table".to_string(),
            details: e.to_string(),
        })?;
        Ok(())
    }

    /// Insert a sampled production turn. `created_at` is stamped by the store.
    pub async fn insert_sample(&self, params: &InsertSampleParams) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let signals_json =
            serde_json::to_string(&params.risk_signals).map_err(|e| SyscityError::Storage {
                context: "Failed to serialize risk signals".to_string(),
                details: e.to_string(),
            })?;

        sqlx::query(
            r#"
            INSERT INTO turn_samples
                (turn_id, session_id, agent_id, conversation_id, input, response, model,
                 cache_hit, total_tokens, latency_ms, verdict, risk_signals, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            "#,
        )
        .bind(&params.turn_id)
        .bind(params.session_id.as_deref())
        .bind(&params.agent_id)
        .bind(&params.conversation_id)
        .bind(&params.input)
        .bind(&params.response)
        .bind(&params.model)
        .bind(params.cache_hit)
        .bind(params.total_tokens as i64)
        .bind(params.latency_ms as i64)
        .bind(params.verdict.as_str())
        .bind(&signals_json)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to insert turn sample".to_string(),
            details: e.to_string(),
        })?;
        Ok(())
    }

    /// List the most recent samples, newest first.
    pub async fn list_recent(&self, limit: u32) -> Result<Vec<TurnSample>> {
        let rows = sqlx::query(
            r#"
            SELECT turn_id, session_id, agent_id, conversation_id, input, response, model,
                   cache_hit, total_tokens, latency_ms, verdict, risk_signals, created_at
            FROM turn_samples
            ORDER BY created_at DESC, rowid DESC
            LIMIT ?1
            "#,
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to list turn samples".to_string(),
            details: e.to_string(),
        })?;

        rows.iter().map(turn_sample_from_row).collect()
    }

    /// Count samples created at or after `since_ms` (unix millis).
    pub async fn count_since(&self, since_ms: i64) -> Result<u64> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM turn_samples WHERE created_at >= ?1")
                .bind(since_ms)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| SyscityError::Storage {
                    context: "Failed to count turn samples".to_string(),
                    details: e.to_string(),
                })?;
        Ok(count as u64)
    }
}

fn turn_sample_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> std::result::Result<TurnSample, SyscityError> {
    use sqlx::Row;
    let verdict: String = row.get("verdict");
    let risk_signals_json: String = row.get("risk_signals");
    let verdict = SampleVerdict::from_str(&verdict).ok_or_else(|| SyscityError::Storage {
        context: "Invalid verdict in turn_samples".to_string(),
        details: format!("unexpected verdict: {verdict}"),
    })?;
    Ok(TurnSample {
        turn_id: row.get("turn_id"),
        session_id: row.get("session_id"),
        agent_id: row.get("agent_id"),
        conversation_id: row.get("conversation_id"),
        input: row.get("input"),
        response: row.get("response"),
        model: row.get("model"),
        cache_hit: row.get("cache_hit"),
        total_tokens: row.get("total_tokens"),
        latency_ms: row.get("latency_ms"),
        verdict,
        risk_signals: serde_json::from_str(&risk_signals_json).unwrap_or_default(),
        created_at: row.get("created_at"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_store() -> TurnSampleStore {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        TurnSampleStore::from_pool(pool).await.unwrap()
    }

    fn params(turn_id: &str, response: &str) -> InsertSampleParams {
        InsertSampleParams {
            turn_id: turn_id.to_string(),
            session_id: Some("s1".into()),
            agent_id: "worker".into(),
            conversation_id: "c1".into(),
            input: "hello".into(),
            response: response.to_string(),
            model: "claude-sonnet-4-6".into(),
            cache_hit: false,
            total_tokens: 123,
            latency_ms: 42,
            verdict: SampleVerdict::Pass,
            risk_signals: vec!["PII detected".to_string()],
        }
    }

    #[tokio::test]
    async fn from_pool_creates_table() {
        let store = test_store().await;
        store.insert_sample(&params("t1", "world")).await.unwrap();
        let rows = store.list_recent(10).await.unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn insert_and_list_recent_roundtrips_newest_first() {
        let store = test_store().await;
        store.insert_sample(&params("t1", "first")).await.unwrap();
        // `created_at` is wall-clock ms; nudge the clock so ordering is
        // deterministic even if both inserts land in the same millisecond.
        std::thread::sleep(std::time::Duration::from_millis(5));
        store.insert_sample(&params("t2", "second")).await.unwrap();

        let rows = store.list_recent(10).await.unwrap();
        assert_eq!(rows.len(), 2);
        // Newest first.
        assert_eq!(rows[0].turn_id, "t2");
        assert_eq!(rows[1].turn_id, "t1");
        let row = &rows[0];
        assert_eq!(row.response, "second");
        assert_eq!(row.session_id.as_deref(), Some("s1"));
        assert_eq!(row.agent_id, "worker");
        assert_eq!(row.model, "claude-sonnet-4-6");
        assert!(!row.cache_hit);
        assert_eq!(row.total_tokens, 123);
        assert_eq!(row.latency_ms, 42);
        assert_eq!(row.verdict, SampleVerdict::Pass);
        assert_eq!(row.risk_signals, vec!["PII detected".to_string()]);
        assert!(row.created_at > 0);
    }

    #[tokio::test]
    async fn list_recent_respects_limit() {
        let store = test_store().await;
        store.insert_sample(&params("t1", "a")).await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        store.insert_sample(&params("t2", "b")).await.unwrap();
        let rows = store.list_recent(1).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].turn_id, "t2");
    }

    #[tokio::test]
    async fn count_since_counts_only_recent_rows() {
        let store = test_store().await;
        store.insert_sample(&params("t1", "a")).await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let boundary = chrono::Utc::now().timestamp_millis();
        std::thread::sleep(std::time::Duration::from_millis(5));
        store.insert_sample(&params("t2", "b")).await.unwrap();

        assert_eq!(store.count_since(0).await.unwrap(), 2);
        assert_eq!(store.count_since(boundary).await.unwrap(), 1);
        assert_eq!(
            store
                .count_since(chrono::Utc::now().timestamp_millis() + 10_000)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn flag_and_error_verdicts_roundtrip() {
        let store = test_store().await;
        let mut p = params("t1", "boom");
        p.verdict = SampleVerdict::Flag;
        store.insert_sample(&p).await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut p2 = params("t2", "errored");
        p2.verdict = SampleVerdict::Error;
        store.insert_sample(&p2).await.unwrap();

        let rows = store.list_recent(10).await.unwrap();
        assert_eq!(rows[0].verdict, SampleVerdict::Error);
        assert_eq!(rows[1].verdict, SampleVerdict::Flag);
    }
}
