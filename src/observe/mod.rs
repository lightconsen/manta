//! Per-turn observability: records, collection, persistence, aggregation.
//!
//! Records one JSON file per agent turn under
//! `~/.syscity/turns/YYYY-MM-DD/<turn_id>.json`, plus metric rows in SQLite
//! (see `agent::session_store::metrics`). Aggregated by `syscity observe`.

pub mod aggregate;
pub mod collector;
pub mod prune;
pub mod record;
pub mod writer;

use std::sync::atomic::AtomicU64;

pub use collector::{TurnContext, TurnMetricsCollector};
pub use record::{
    ErrorSource, LlmRoundRecord, ObservedError, ObservedToolCall, ObservedUsage, TurnEndState,
    TurnRecord,
};
pub use writer::TurnMetricsWriter;

/// Optional numeric sink for turn metrics (SQLite rows). The JSON writer always
/// runs; when a sink is attached, the collector additionally persists the
/// aggregateable numbers (see `session_store::metrics`).
///
/// A boxed future keeps the trait dyn-compatible (RPITIT `async fn` would not
/// be).
pub trait TurnMetricsSink: Send + Sync {
    /// Persist the numeric metrics of a completed turn record.
    fn persist_turn<'a>(
        &'a self,
        rec: &'a TurnRecord,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::Result<()>> + Send + 'a>>;
}

/// Count of observability record write failures (surfaced via metrics).
pub static WRITE_FAILURES: AtomicU64 = AtomicU64::new(0);
