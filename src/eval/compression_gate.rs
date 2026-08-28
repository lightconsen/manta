//! Compression low-retention quality gate (Wave 2 — Agent 2).
//!
//! When an agent's context compression repeatedly drops below the configured
//! retention ratio, the online pipeline flags each occurrence as an
//! `online:risk` pending badcase carrying the "context compression low
//! retention" risk signal. A burst of those flags inside a short window means
//! the model is silently shedding context — a release-blocking regression.
//!
//! This module counts those pending rows and evaluates the criterion: the gate
//! passes iff the count does not exceed `max_flagged_in_window`.

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::eval::pending_badcase::{PendingBadcaseStore, PendingSource, PendingStatus};
use crate::gateway::quality_gate::CriterionResult;

/// Stable prefix of the low-retention risk signal the engine writes into a
/// pending badcase's `risk_signals`.
///
/// The full signal is formatted by `observe::collector` as
/// `"context compression low retention (ratio={:.3}, strategy={}, tokens {}→{})"`.
/// We match on the invariant prefix so the check is robust to the variable
/// ratio / strategy / token counts.
pub const LOW_RETENTION_SIGNAL_PREFIX: &str = "context compression low retention (ratio=";

/// Configuration for the compression low-retention gate.
///
/// Default-disabled: an unconfigured gate never blocks a release.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompressionGateConfig {
    pub enabled: bool,
    pub max_flagged_in_window: u32,
    pub window_ms: i64,
}

/// Count pending `online:risk` badcases flagged for low retention within the
/// window and evaluate the gate.
///
/// Returns `None` when the gate is disabled. Otherwise returns a
/// [`CriterionResult`] where `passed` is `true` iff the number of pending
/// `online:risk` rows carrying the low-retention signal with
/// `created_at >= now_ms - window_ms` does not exceed `max_flagged_in_window`.
pub async fn compression_criterion(
    store: &PendingBadcaseStore,
    cfg: &CompressionGateConfig,
    now_ms: i64,
) -> Option<CriterionResult> {
    if !cfg.enabled {
        return None;
    }

    let cutoff = now_ms.saturating_sub(cfg.window_ms);
    let pending = match store.list_pending(PendingStatus::Pending, u32::MAX).await {
        Ok(rows) => rows,
        Err(e) => {
            // Fail closed: a gate that cannot read the pool must not pass.
            warn!(error = %e, "compression gate: failed to list pending badcases");
            return Some(CriterionResult {
                criterion: "compression_low_retention".to_string(),
                passed: false,
                actual: 0.0,
                threshold: cfg.max_flagged_in_window as f64,
                detail: format!("failed to read pending badcases: {e}"),
            });
        }
    };

    let count = pending
        .iter()
        .filter(|row| {
            row.status == PendingStatus::Pending
                && row.source == PendingSource::OnlineRisk
                && row
                    .risk_signals
                    .iter()
                    .any(|s| s.contains(LOW_RETENTION_SIGNAL_PREFIX))
                && row.created_at >= cutoff
        })
        .count();

    let passed = count <= cfg.max_flagged_in_window as usize;
    Some(CriterionResult {
        criterion: "compression_low_retention".to_string(),
        passed,
        actual: count as f64,
        threshold: cfg.max_flagged_in_window as f64,
        detail: format!("{} low-retention online-risk flags in last {} ms", count, cfg.window_ms),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::pending_badcase::{dedup_hash, InsertPendingParams};

    /// Build an in-memory store, returning both the raw pool (for direct SQL
    /// inserts that control `created_at`) and the store built on it.
    async fn test_store() -> (sqlx::Pool<sqlx::Sqlite>, PendingBadcaseStore) {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        let store = PendingBadcaseStore::from_pool(pool.clone()).await.unwrap();
        (pool, store)
    }

    /// The full low-retention signal as the engine would format it.
    fn low_retention_signal() -> String {
        format!(
            "{}0.100, strategy=heuristic_summary, tokens 5000→500)",
            LOW_RETENTION_SIGNAL_PREFIX
        )
    }

    fn risk_params(input: &str) -> InsertPendingParams {
        InsertPendingParams {
            source: PendingSource::OnlineRisk,
            turn_id: Some("t".into()),
            session_id: None,
            agent_id: None,
            input: input.to_string(),
            response: format!("response-{input}"),
            risk_signals: vec![low_retention_signal()],
        }
    }

    /// Insert a row with full control over source/status/created_at. The public
    /// `insert_pending` fixes `created_at` to "now", so old rows and non-pending
    /// statuses are written directly.
    async fn insert_raw(
        pool: &sqlx::Pool<sqlx::Sqlite>,
        id: &str,
        source: &str,
        status: &str,
        created_at: i64,
    ) {
        let signals = serde_json::to_string(&vec![low_retention_signal()]).unwrap();
        let hash = dedup_hash(id, id);
        sqlx::query(
            r#"
            INSERT INTO pending_badcases
                (id, source, turn_id, session_id, agent_id, input, response, risk_signals, dedup_hash, status, created_at, updated_at)
            VALUES (?1, ?2, NULL, NULL, NULL, ?3, ?3, ?4, ?5, ?6, ?7, ?7)
            "#,
        )
        .bind(id)
        .bind(source)
        .bind(id) // input == id
        .bind(signals)
        .bind(hash)
        .bind(status)
        .bind(created_at)
        .execute(pool)
        .await
        .unwrap();
    }

    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    #[tokio::test]
    async fn disabled_config_returns_none() {
        let (_pool, store) = test_store().await;
        store.insert_pending(&risk_params("in-1")).await.unwrap();
        let cfg = CompressionGateConfig::default();
        let result = compression_criterion(&store, &cfg, now_ms()).await;
        assert!(result.is_none(), "disabled gate must yield no criterion");
    }

    #[tokio::test]
    async fn flags_over_threshold_fail_the_gate() {
        let (_pool, store) = test_store().await;
        store.insert_pending(&risk_params("in-1")).await.unwrap();
        store.insert_pending(&risk_params("in-2")).await.unwrap();
        store.insert_pending(&risk_params("in-3")).await.unwrap();

        let cfg = CompressionGateConfig {
            enabled: true,
            max_flagged_in_window: 2,
            window_ms: 60_000,
        };
        let result = compression_criterion(&store, &cfg, now_ms()).await.unwrap();
        assert!(!result.passed, "3 flags must fail a 2-flag gate");
        assert_eq!(result.criterion, "compression_low_retention");
        assert_eq!(result.actual, 3.0);
        assert_eq!(result.threshold, 2.0);
        assert!(result.detail.contains("3 low-retention"));
    }

    #[tokio::test]
    async fn flags_at_or_below_threshold_pass() {
        let (_pool, store) = test_store().await;
        store.insert_pending(&risk_params("in-1")).await.unwrap();
        store.insert_pending(&risk_params("in-2")).await.unwrap();
        store.insert_pending(&risk_params("in-3")).await.unwrap();

        // Equal to the threshold → pass.
        let cfg = CompressionGateConfig {
            enabled: true,
            max_flagged_in_window: 3,
            window_ms: 60_000,
        };
        let result = compression_criterion(&store, &cfg, now_ms()).await.unwrap();
        assert!(result.passed);
        assert_eq!(result.actual, 3.0);

        // Below the threshold → pass.
        let cfg = CompressionGateConfig {
            enabled: true,
            max_flagged_in_window: 10,
            window_ms: 60_000,
        };
        let result = compression_criterion(&store, &cfg, now_ms()).await.unwrap();
        assert!(result.passed);
        assert_eq!(result.actual, 3.0);
    }

    #[tokio::test]
    async fn rows_older_than_window_are_not_counted() {
        let (pool, store) = test_store().await;
        let now = now_ms();
        let window_ms = 60_000;
        // Two rows created well before the window opened.
        insert_raw(&pool, "old-1", "online:risk", "pending", now - 2 * window_ms).await;
        insert_raw(&pool, "old-2", "online:risk", "pending", now - 2 * window_ms).await;
        // One row inside the window.
        store.insert_pending(&risk_params("fresh")).await.unwrap();

        // max=0 → only the in-window row counts, so the gate fails.
        let cfg = CompressionGateConfig {
            enabled: true,
            max_flagged_in_window: 0,
            window_ms,
        };
        let result = compression_criterion(&store, &cfg, now).await.unwrap();
        assert!(!result.passed);
        assert_eq!(result.actual, 1.0, "only the fresh row should count");
    }

    #[tokio::test]
    async fn only_old_rows_pass_a_zero_threshold() {
        let (pool, store) = test_store().await;
        let now = now_ms();
        insert_raw(&pool, "old-1", "online:risk", "pending", now - 200_000).await;

        let cfg = CompressionGateConfig {
            enabled: true,
            max_flagged_in_window: 0,
            window_ms: 60_000,
        };
        let result = compression_criterion(&store, &cfg, now).await.unwrap();
        assert!(result.passed);
        assert_eq!(result.actual, 0.0);
    }

    #[tokio::test]
    async fn only_pending_online_risk_rows_count() {
        let (pool, store) = test_store().await;
        let now = now_ms();
        // Confirmed OnlineRisk low-retention row → excluded by status.
        insert_raw(&pool, "confirmed-1", "online:risk", "confirmed", now).await;
        // HumanDislike low-retention row → excluded by source.
        insert_raw(&pool, "dislike-1", "human:dislike", "pending", now).await;

        let cfg = CompressionGateConfig {
            enabled: true,
            max_flagged_in_window: 0,
            window_ms: 60_000,
        };
        let result = compression_criterion(&store, &cfg, now).await.unwrap();
        assert!(result.passed);
        assert_eq!(result.actual, 0.0);
    }

    #[tokio::test]
    async fn rows_without_low_retention_signal_are_not_counted() {
        let (_pool, store) = test_store().await;
        store
            .insert_pending(&InsertPendingParams {
                source: PendingSource::OnlineRisk,
                turn_id: None,
                session_id: None,
                agent_id: None,
                input: "noise".into(),
                response: "noise-r".into(),
                risk_signals: vec!["PII detected".to_string()],
            })
            .await
            .unwrap();

        let cfg = CompressionGateConfig {
            enabled: true,
            max_flagged_in_window: 0,
            window_ms: 60_000,
        };
        let result = compression_criterion(&store, &cfg, now_ms()).await.unwrap();
        assert!(result.passed);
        assert_eq!(result.actual, 0.0);
    }
}
