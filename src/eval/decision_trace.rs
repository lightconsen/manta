//! Decision-trace store.
//!
//! Every tuning decision made by the harness — applying/rejecting an
//! optimizer candidate, a guardrail rollback, a shadow-eval gate pass/fail —
//! is recorded here so the loop is auditable and replayable (§十二 可追溯).

use std::time::Duration;

use serde_json::Value;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Pool, Sqlite};
use tracing::{info, instrument};

use crate::error::{Result, SyscityError};

/// What kind of decision this trace records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceKind {
    /// An optimizer candidate was applied (CAS success).
    OptimizerApply,
    /// An optimizer candidate was rejected (conflict, guardrail, regression).
    OptimizerReject,
    /// A previously applied change was rolled back.
    Rollback,
    /// A shadow-eval / quality gate passed a candidate.
    GatePass,
    /// A shadow-eval / quality gate failed a candidate.
    GateFail,
}

impl TraceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TraceKind::OptimizerApply => "optimizer_apply",
            TraceKind::OptimizerReject => "optimizer_reject",
            TraceKind::Rollback => "rollback",
            TraceKind::GatePass => "gate_pass",
            TraceKind::GateFail => "gate_fail",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "optimizer_apply" => Some(TraceKind::OptimizerApply),
            "optimizer_reject" => Some(TraceKind::OptimizerReject),
            "rollback" => Some(TraceKind::Rollback),
            "gate_pass" => Some(TraceKind::GatePass),
            "gate_fail" => Some(TraceKind::GateFail),
            _ => None,
        }
    }
}

/// Lifecycle status of a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceStatus {
    /// Decided but not yet applied (e.g. awaiting guardrails).
    Pending,
    /// The change was applied to the live configuration.
    Applied,
    /// The change was rejected / superseded.
    Rejected,
}

impl TraceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TraceStatus::Pending => "pending",
            TraceStatus::Applied => "applied",
            TraceStatus::Rejected => "rejected",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(TraceStatus::Pending),
            "applied" => Some(TraceStatus::Applied),
            "rejected" => Some(TraceStatus::Rejected),
            _ => None,
        }
    }
}

/// A single recorded decision.
#[derive(Debug, Clone)]
pub struct DecisionTrace {
    pub id: String,
    pub kind: TraceKind,
    /// The tuning subject (e.g. a config path like `agent.default.temperature`).
    pub subject: String,
    /// JSON snapshot of the proposed change.
    pub payload: Value,
    /// JSON evidence backing the decision (verdict, score, CI results).
    pub evidence: Value,
    pub status: TraceStatus,
    pub decided_at: i64,
    pub applied_at: Option<i64>,
}

/// Parameters for [`DecisionTraceStore::record`].
#[derive(Debug, Clone)]
pub struct RecordTraceParams {
    pub kind: TraceKind,
    pub subject: String,
    pub payload: Value,
    pub evidence: Value,
    pub status: TraceStatus,
}

/// SQLite-backed audit log for harness tuning decisions.
#[derive(Debug, Clone)]
pub struct DecisionTraceStore {
    pool: Pool<Sqlite>,
}

impl DecisionTraceStore {
    /// Create a new decision-trace store from a database URL.
    #[instrument(skip(database_url))]
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(30))
            .connect(database_url)
            .await
            .map_err(|e| SyscityError::Storage {
                context: "Failed to connect to decision-trace database".to_string(),
                details: e.to_string(),
            })?;
        Self::from_pool(pool).await
    }

    /// Create a decision-trace store from an existing connection pool.
    ///
    /// Must be handed the same pool shared by the other stores (not a fresh
    /// `:memory:` connection) so all tables live in one database.
    pub async fn from_pool(pool: Pool<Sqlite>) -> Result<Self> {
        let store = Self { pool };
        store.optimize().await?;
        store.init_schema().await?;
        info!("Decision-trace store initialized");
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

    /// Initialize the `decision_traces` table.
    async fn init_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS decision_traces (
                id          TEXT PRIMARY KEY,
                kind        TEXT NOT NULL,
                subject     TEXT NOT NULL,
                payload     TEXT NOT NULL,
                evidence    TEXT NOT NULL,
                status      TEXT NOT NULL,
                decided_at  INTEGER NOT NULL,
                applied_at  INTEGER
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to create decision_traces table".to_string(),
            details: e.to_string(),
        })?;
        Ok(())
    }

    /// Record a decision. `applied_at` is set when `status` is `Applied`.
    pub async fn record(&self, params: &RecordTraceParams) -> Result<DecisionTrace> {
        let now = chrono::Utc::now().timestamp_millis();
        let id = uuid::Uuid::new_v4().to_string();
        let applied_at = if params.status == TraceStatus::Applied {
            Some(now)
        } else {
            None
        };
        sqlx::query(
            r#"
            INSERT INTO decision_traces (id, kind, subject, payload, evidence, status, decided_at, applied_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
        )
        .bind(&id)
        .bind(params.kind.as_str())
        .bind(&params.subject)
        .bind(serde_json::to_string(&params.payload).unwrap_or_else(|_| "{}".to_string()))
        .bind(serde_json::to_string(&params.evidence).unwrap_or_else(|_| "{}".to_string()))
        .bind(params.status.as_str())
        .bind(now)
        .bind(applied_at)
        .execute(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to record decision trace".to_string(),
            details: e.to_string(),
        })?;

        Ok(DecisionTrace {
            id,
            kind: params.kind,
            subject: params.subject.clone(),
            payload: params.payload.clone(),
            evidence: params.evidence.clone(),
            status: params.status,
            decided_at: now,
            applied_at,
        })
    }

    /// Update a trace's status after the fact (e.g. pending → applied).
    pub async fn update_status(&self, id: &str, status: TraceStatus) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let applied_at = if status == TraceStatus::Applied {
            Some(now)
        } else {
            None
        };
        sqlx::query(
            "UPDATE decision_traces SET status = ?1, applied_at = COALESCE(?2, applied_at) WHERE id = ?3",
        )
        .bind(status.as_str())
        .bind(applied_at)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| SyscityError::Storage {
            context: format!("Failed to update decision trace {id}"),
            details: e.to_string(),
        })?;
        Ok(())
    }

    /// List traces, newest first, optionally filtered by kind.
    pub async fn list(&self, kind: Option<TraceKind>, limit: u32) -> Result<Vec<DecisionTrace>> {
        let rows = if let Some(kind) = kind {
            sqlx::query(
                r#"
                SELECT id, kind, subject, payload, evidence, status, decided_at, applied_at
                FROM decision_traces
                WHERE kind = ?1
                ORDER BY decided_at DESC, rowid DESC
                LIMIT ?2
                "#,
            )
            .bind(kind.as_str())
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                r#"
                SELECT id, kind, subject, payload, evidence, status, decided_at, applied_at
                FROM decision_traces
                ORDER BY decided_at DESC, rowid DESC
                LIMIT ?1
                "#,
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| SyscityError::Storage {
            context: "Failed to list decision traces".to_string(),
            details: e.to_string(),
        })?;

        rows.iter().map(decision_trace_from_row).collect()
    }
}

fn decision_trace_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> std::result::Result<DecisionTrace, SyscityError> {
    use sqlx::Row;
    let id: String = row.get("id");
    let kind: String = row.get("kind");
    let subject: String = row.get("subject");
    let payload: String = row.get("payload");
    let evidence: String = row.get("evidence");
    let status: String = row.get("status");
    let decided_at: i64 = row.get("decided_at");
    let applied_at: Option<i64> = row.get("applied_at");

    let kind = TraceKind::from_str(&kind).ok_or_else(|| SyscityError::Storage {
        context: "Invalid kind in decision_traces".to_string(),
        details: format!("unexpected kind: {kind}"),
    })?;
    let status = TraceStatus::from_str(&status).ok_or_else(|| SyscityError::Storage {
        context: "Invalid status in decision_traces".to_string(),
        details: format!("unexpected status: {status}"),
    })?;

    Ok(DecisionTrace {
        id,
        kind,
        subject,
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        evidence: serde_json::from_str(&evidence).unwrap_or(Value::Null),
        status,
        decided_at,
        applied_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_store() -> DecisionTraceStore {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        DecisionTraceStore::from_pool(pool).await.unwrap()
    }

    fn params(kind: TraceKind, subject: &str, status: TraceStatus) -> RecordTraceParams {
        RecordTraceParams {
            kind,
            subject: subject.to_string(),
            payload: serde_json::json!({ "temperature": 0.7 }),
            evidence: serde_json::json!({ "score": 0.9, "verdict": "improved" }),
            status,
        }
    }

    #[tokio::test]
    async fn record_then_list_roundtrips() {
        let store = test_store().await;
        store
            .record(&params(
                TraceKind::OptimizerApply,
                "agent.default.temperature",
                TraceStatus::Applied,
            ))
            .await
            .unwrap();
        store
            .record(&params(TraceKind::Rollback, "agent.default.temperature", TraceStatus::Applied))
            .await
            .unwrap();

        let all = store.list(None, 10).await.unwrap();
        assert_eq!(all.len(), 2);
        // Newest first.
        assert_eq!(all[0].kind, TraceKind::Rollback);
        assert_eq!(all[1].kind, TraceKind::OptimizerApply);
        assert_eq!(all[1].subject, "agent.default.temperature");
        assert_eq!(all[1].payload["temperature"], 0.7);
        assert!(all[1].applied_at.is_some());
    }

    #[tokio::test]
    async fn list_filters_by_kind() {
        let store = test_store().await;
        store
            .record(&params(TraceKind::GatePass, "prompt:v1", TraceStatus::Pending))
            .await
            .unwrap();
        store
            .record(&params(TraceKind::GateFail, "prompt:v2", TraceStatus::Rejected))
            .await
            .unwrap();

        let fails = store.list(Some(TraceKind::GateFail), 10).await.unwrap();
        assert_eq!(fails.len(), 1);
        assert_eq!(fails[0].subject, "prompt:v2");
        assert_eq!(fails[0].status, TraceStatus::Rejected);
    }

    #[tokio::test]
    async fn update_status_sets_applied_at() {
        let store = test_store().await;
        let trace = store
            .record(&params(TraceKind::OptimizerApply, "a", TraceStatus::Pending))
            .await
            .unwrap();
        assert!(trace.applied_at.is_none());

        store
            .update_status(&trace.id, TraceStatus::Applied)
            .await
            .unwrap();
        let traces = store.list(None, 10).await.unwrap();
        let updated = &traces[0];
        assert_eq!(updated.status, TraceStatus::Applied);
        assert!(updated.applied_at.is_some());
    }
}
