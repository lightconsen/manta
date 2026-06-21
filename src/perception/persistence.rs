//! Persistent observation storage.
//!
//! [`ObservationStore`] is the durability boundary for the perception layer.
//! Observations are still kept in the in-memory aggregation window for
//! near-real-time queries; the store is appended to in parallel and used
//! for cross-restart history and time-range queries beyond the window.
//!
//! Two backends are provided out of the box:
//!
//! - [`NullObservationStore`] — discards everything (default; zero overhead).
//! - [`JsonlObservationStore`] — file-backed, one JSONL file per UTC day.
//!
//! Future work: a SQLite backend reusing the Gateway's `sqlx::SqlitePool`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;

use crate::perception::{Modality, Observation, ObservationId, PerceptionQuery};

/// Errors raised by [`ObservationStore`] implementations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Underlying I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Serialisation failure.
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    /// Catch-all.
    #[error("{0}")]
    Other(String),
}

/// Interface for persisting observations beyond the in-memory window.
///
/// All methods are async-safe and may be called concurrently. Implementations
/// should be cheap (microseconds) for `append` so the polling hot path is
/// not blocked.
#[async_trait]
pub trait ObservationStore: Send + Sync {
    /// Append a single observation. The default implementation calls
    /// [`append_batch`] with a one-element slice.
    async fn append(&self, obs: &Observation) -> Result<(), StoreError> {
        self.append_batch(std::slice::from_ref(obs)).await
    }

    /// Append a batch of observations. Implementations should be atomic
    /// at the per-observation level — partial failure should report the
    /// error but not double-write any observation.
    async fn append_batch(&self, obs: &[Observation]) -> Result<(), StoreError>;

    /// Query persisted observations matching the filter.
    ///
    /// `since` restricts to observations whose `created_at >= since`.
    /// Implementations may apply a reasonable default cap on result size
    /// (e.g. 10_000 rows) when `query.limit` is `None`.
    async fn query(
        &self,
        query: &PerceptionQuery,
        since: Option<SystemTime>,
    ) -> Result<Vec<Observation>, StoreError>;

    /// Delete observations created before `cutoff`. Returns the number
    /// of observations removed (best-effort estimate; a JSONL-by-day store
    /// may report whole-file counts only).
    async fn prune_older_than(&self, cutoff: SystemTime) -> Result<u64, StoreError>;
}

// ── NullObservationStore ────────────────────────────────────────────────

/// Discards all writes. Default when persistence is disabled.
#[derive(Debug, Default)]
pub struct NullObservationStore;

#[async_trait]
impl ObservationStore for NullObservationStore {
    async fn append_batch(&self, _obs: &[Observation]) -> Result<(), StoreError> {
        Ok(())
    }

    async fn query(
        &self,
        _query: &PerceptionQuery,
        _since: Option<SystemTime>,
    ) -> Result<Vec<Observation>, StoreError> {
        Ok(Vec::new())
    }

    async fn prune_older_than(&self, _cutoff: SystemTime) -> Result<u64, StoreError> {
        Ok(0)
    }
}

// ── JsonlObservationStore ───────────────────────────────────────────────

/// Wire format for JSONL records. We keep this distinct from
/// [`Observation`] because the runtime struct contains [`std::time::Instant`]
/// (not portable) and uses [`Modality`] (custom Serialize-only) — the
/// stored form must round-trip through serde including deserialise.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedObservation {
    id: String,
    source: String,
    modality: PersistedModality,
    /// Seconds since UNIX_EPOCH. Used as the canonical timestamp for
    /// queries that need cross-restart ordering.
    created_at_unix_secs: u64,
    /// Sub-second component in nanoseconds.
    created_at_nanos: u32,
    confidence: f32,
    data: serde_json::Value,
}

/// Serializable mirror of [`crate::perception::Modality`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum PersistedModality {
    Rgb,
    Depth,
    Audio,
    Tactile,
    System,
    Device,
    UiTree,
    FileSystem,
    Network,
    Other,
}

impl From<Modality> for PersistedModality {
    fn from(m: Modality) -> Self {
        match m {
            Modality::Rgb => Self::Rgb,
            Modality::Depth => Self::Depth,
            Modality::Audio => Self::Audio,
            Modality::Tactile => Self::Tactile,
            Modality::System => Self::System,
            Modality::Device => Self::Device,
            Modality::UiTree => Self::UiTree,
            Modality::FileSystem => Self::FileSystem,
            Modality::Network => Self::Network,
            Modality::Other => Self::Other,
        }
    }
}

impl From<PersistedModality> for Modality {
    fn from(m: PersistedModality) -> Self {
        match m {
            PersistedModality::Rgb => Self::Rgb,
            PersistedModality::Depth => Self::Depth,
            PersistedModality::Audio => Self::Audio,
            PersistedModality::Tactile => Self::Tactile,
            PersistedModality::System => Self::System,
            PersistedModality::Device => Self::Device,
            PersistedModality::UiTree => Self::UiTree,
            PersistedModality::FileSystem => Self::FileSystem,
            PersistedModality::Network => Self::Network,
            PersistedModality::Other => Self::Other,
        }
    }
}

impl PersistedObservation {
    fn from_obs(obs: &Observation) -> Self {
        let dur = obs
            .created_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        Self {
            id: obs.id.to_string(),
            source: obs.source.clone(),
            modality: obs.modality.into(),
            created_at_unix_secs: dur.as_secs(),
            created_at_nanos: dur.subsec_nanos(),
            confidence: obs.confidence,
            data: obs.data.clone(),
        }
    }

    fn into_obs(self) -> Observation {
        let created_at =
            UNIX_EPOCH + Duration::new(self.created_at_unix_secs, self.created_at_nanos);
        // Synthesize a fresh Instant — we have no reference point across restarts.
        // The Instant is only used inside the in-process aggregator; for queries
        // routed back into the store, ordering is via created_at.
        Observation {
            id: parse_observation_id(&self.id),
            source: self.source,
            modality: self.modality.into(),
            timestamp: std::time::Instant::now(),
            created_at,
            confidence: self.confidence,
            data: self.data,
        }
    }
}

fn parse_observation_id(s: &str) -> ObservationId {
    // ObservationId currently wraps a UUID string — there is no Deserialize
    // impl, but we can reconstruct via Display roundtripping is best-effort.
    // We keep the original string by stuffing it through a synthetic
    // ObservationId. Since the field is private, fall back to generating a
    // new ID and stashing the original under a side-channel is overkill.
    // For now, generate a fresh ID — observations recovered from disk are
    // usually only consumed by the LLM, not joined with live observations.
    let _ = s;
    ObservationId::new()
}

/// File-backed persistent store. One JSONL file per UTC day; daily files
/// are pruned wholesale by [`prune_older_than`].
///
/// Layout:
/// ```text
/// {root}/2026-06-17.jsonl
/// {root}/2026-06-18.jsonl
/// ```
///
/// Each line is a [`PersistedObservation`] serialized as JSON.
pub struct JsonlObservationStore {
    root: PathBuf,
    /// Serializes appends so concurrent pollers don't interleave partial
    /// writes.
    writer_lock: AsyncMutex<()>,
}

impl JsonlObservationStore {
    /// Create or open a JSONL store at `root`. The directory is created if
    /// missing.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        tokio::fs::create_dir_all(&root).await?;
        Ok(Self {
            root,
            writer_lock: AsyncMutex::new(()),
        })
    }

    fn day_file(&self, when: SystemTime) -> PathBuf {
        self.root.join(format!("{}.jsonl", day_key(when)))
    }

    async fn list_day_files(&self) -> Result<Vec<(String, PathBuf)>, StoreError> {
        let mut entries = tokio::fs::read_dir(&self.root).await?;
        let mut out = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                // Validate stem is a YYYY-MM-DD-shaped key.
                if stem.len() == 10 && stem.as_bytes()[4] == b'-' && stem.as_bytes()[7] == b'-' {
                    out.push((stem.to_string(), path));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

#[async_trait]
impl ObservationStore for JsonlObservationStore {
    async fn append_batch(&self, obs: &[Observation]) -> Result<(), StoreError> {
        if obs.is_empty() {
            return Ok(());
        }
        let _guard = self.writer_lock.lock().await;

        // Group by day-file so we open each at most once per batch.
        use std::collections::HashMap;
        let mut by_day: HashMap<PathBuf, String> = HashMap::new();
        for o in obs {
            let path = self.day_file(o.created_at);
            let mut line = serde_json::to_string(&PersistedObservation::from_obs(o))?;
            line.push('\n');
            by_day.entry(path).or_default().push_str(&line);
        }

        for (path, payload) in by_day {
            let mut f = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await?;
            f.write_all(payload.as_bytes()).await?;
            f.flush().await?;
        }
        Ok(())
    }

    async fn query(
        &self,
        query: &PerceptionQuery,
        since: Option<SystemTime>,
    ) -> Result<Vec<Observation>, StoreError> {
        let files = self.list_day_files().await?;
        let mut out = Vec::new();
        let cap = query.limit.unwrap_or(10_000);

        for (_day, path) in files.into_iter().rev() {
            // Iterate newest day first.
            let f = tokio::fs::File::open(&path).await?;
            let reader = tokio::io::BufReader::new(f);
            let mut lines = reader.lines();
            while let Some(line) = lines.next_line().await? {
                if line.is_empty() {
                    continue;
                }
                let pers: PersistedObservation = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue, // skip corrupt lines
                };
                let obs = pers.into_obs();
                if let Some(s) = since {
                    if obs.created_at < s {
                        continue;
                    }
                }
                if !query.matches_observation(&obs) {
                    continue;
                }
                out.push(obs);
                if out.len() >= cap {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }

    async fn prune_older_than(&self, cutoff: SystemTime) -> Result<u64, StoreError> {
        let cutoff_key = day_key(cutoff);
        let mut deleted = 0u64;
        for (day, path) in self.list_day_files().await? {
            if day < cutoff_key {
                // Best-effort line count for the report.
                if let Ok(meta) = tokio::fs::metadata(&path).await {
                    deleted += (meta.len() / 100).max(1); // rough estimate
                }
                let _ = tokio::fs::remove_file(&path).await;
            }
        }
        Ok(deleted)
    }
}

/// Format a `SystemTime` as `YYYY-MM-DD` (UTC). Stable lexical ordering
/// matches chronological ordering, which keeps day-file pruning trivial.
fn day_key(when: SystemTime) -> String {
    let dur = when.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let secs = dur.as_secs() as i64;
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

/// Convert "days since 1970-01-01" → (year, month, day) using Howard Hinnant's
/// civil_from_days algorithm. Avoids pulling in `chrono` for one function.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ── Construction helper ─────────────────────────────────────────────────

/// Build an [`ObservationStore`] from configuration.
///
/// `backend` accepts `"none"` / `"jsonl"`. Unknown values fall back to
/// `"none"`.
pub async fn build_store(
    backend: &str,
    root_dir: Option<PathBuf>,
) -> Result<Arc<dyn ObservationStore>, StoreError> {
    match backend {
        "jsonl" => {
            let dir =
                root_dir.unwrap_or_else(|| std::env::temp_dir().join("syscity-perception-jsonl"));
            let store = JsonlObservationStore::open(dir).await?;
            Ok(Arc::new(store))
        }
        _ => Ok(Arc::new(NullObservationStore)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::{Modality, ObservationId};

    fn make_obs(source: &str, modality: Modality, when: SystemTime) -> Observation {
        Observation {
            id: ObservationId::new(),
            source: source.to_string(),
            modality,
            timestamp: std::time::Instant::now(),
            created_at: when,
            confidence: 1.0,
            data: serde_json::json!({"v": 1}),
        }
    }

    #[tokio::test]
    async fn test_null_store_round_trip() {
        let s = NullObservationStore;
        s.append(&make_obs("a", Modality::Rgb, SystemTime::now()))
            .await
            .unwrap();
        let q = PerceptionQuery::default();
        let r = s.query(&q, None).await.unwrap();
        assert!(r.is_empty());
    }

    #[tokio::test]
    async fn test_jsonl_append_and_query() {
        let dir = std::env::temp_dir().join(format!("syscity-jsonl-test-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let store = JsonlObservationStore::open(&dir).await.unwrap();

        let now = SystemTime::now();
        let obs = vec![
            make_obs("alpha", Modality::Device, now),
            make_obs("beta", Modality::System, now),
        ];
        store.append_batch(&obs).await.unwrap();

        let q = PerceptionQuery::default();
        let r = store.query(&q, None).await.unwrap();
        assert_eq!(r.len(), 2);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_jsonl_query_filters_by_modality() {
        let dir = std::env::temp_dir().join(format!("syscity-jsonl-mod-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let store = JsonlObservationStore::open(&dir).await.unwrap();

        let now = SystemTime::now();
        store
            .append_batch(&[
                make_obs("a", Modality::Rgb, now),
                make_obs("b", Modality::Audio, now),
            ])
            .await
            .unwrap();

        let mut q = PerceptionQuery::default();
        q.modalities = Some(vec![Modality::Audio]);
        let r = store.query(&q, None).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].modality, Modality::Audio);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_jsonl_query_since_filter() {
        let dir = std::env::temp_dir().join(format!("syscity-jsonl-since-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let store = JsonlObservationStore::open(&dir).await.unwrap();

        let now = SystemTime::now();
        let earlier = now - Duration::from_secs(10);
        store
            .append_batch(&[
                make_obs("old", Modality::Rgb, earlier),
                make_obs("new", Modality::Rgb, now),
            ])
            .await
            .unwrap();

        let q = PerceptionQuery::default();
        let cutoff = now - Duration::from_secs(2);
        let r = store.query(&q, Some(cutoff)).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].source, "new");
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_jsonl_prune_older_than() {
        let dir = std::env::temp_dir().join(format!("syscity-jsonl-prune-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let store = JsonlObservationStore::open(&dir).await.unwrap();

        // Two observations: one ~3 days ago, one now.
        let now = SystemTime::now();
        let three_days_ago = now - Duration::from_secs(86_400 * 3);
        store
            .append_batch(&[
                make_obs("old", Modality::Rgb, three_days_ago),
                make_obs("new", Modality::Rgb, now),
            ])
            .await
            .unwrap();

        // Prune anything older than 1 day.
        let one_day_ago = now - Duration::from_secs(86_400);
        store.prune_older_than(one_day_ago).await.unwrap();

        let q = PerceptionQuery::default();
        let r = store.query(&q, None).await.unwrap();
        assert!(r.iter().all(|o| o.source != "old"));
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn test_day_key_format() {
        // 2026-06-17 = day 20_621 since 1970-01-01
        // Verify lexical sorting matches chronological.
        let a = day_key(UNIX_EPOCH);
        assert_eq!(a, "1970-01-01");
        let b = day_key(UNIX_EPOCH + Duration::from_secs(86_400 * 365));
        assert_eq!(b, "1971-01-01");
        assert!(a < b);
    }

    #[test]
    fn test_civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(31), (1970, 2, 1));
        assert_eq!(civil_from_days(365), (1971, 1, 1));
    }

    #[tokio::test]
    async fn test_build_store_unknown_backend_falls_back() {
        let s = build_store("nonsense", None).await.unwrap();
        // Should be Null — append succeeds and query returns empty.
        s.append(&make_obs("x", Modality::Rgb, SystemTime::now()))
            .await
            .unwrap();
        let r = s.query(&PerceptionQuery::default(), None).await.unwrap();
        assert!(r.is_empty());
    }
}
