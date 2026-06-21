//! Sliding-window temporal aggregation per `(source, modality)`.
//!
//! [`TemporalProcessor`] is the **shared** stage of the perception
//! pipeline that maintains rolling statistics over the recent
//! observation history. It does **not** emit events — `Event::Change`
//! and `Event::Discrete` are produced per-agent by
//! [`super::SalienceFilter`], which has different thresholds for
//! different agents. The processor's only job is to give every agent
//! a cheap, consistent view of the world via
//! [`TemporalProcessor::snapshot_aggregates`], read by
//! [`super::AgentPerceptionAdapter::now`].
//!
//! # Design
//!
//! Per `(source, modality)` we keep:
//!
//! - `count` / `first_at` / `last_at` — observation cadence
//! - `last_payload` — most recent raw `data` payload
//! - Optional numeric series stats — `min`, `max`, `mean` — populated when
//!   [`extract_scalar`] succeeds (numeric leaf or top-level scalar field like
//!   `"value"` / `"pct"` / `"rms"`)
//!
//! The window is a wall-clock duration. Old samples are evicted
//! lazily on each push.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::perception::{Aggregate, Modality, Observation, PerceptionStreamHub};

/// Default rolling-window duration for aggregates.
pub const DEFAULT_TEMPORAL_WINDOW: Duration = Duration::from_secs(10);

/// Read-only access to current sliding-window aggregates.
///
/// Implementations are `Send + Sync` and cheap to call concurrently —
/// the snapshot is a clone of the current state.
#[async_trait]
pub trait TemporalProcessor: Send + Sync {
    /// Snapshot current aggregates keyed by `(source, modality)`.
    fn snapshot_aggregates(&self) -> HashMap<(String, Modality), Aggregate>;

    /// Manually push an observation (used by spawn-less consumers, e.g.
    /// tests or replay). Production code uses
    /// [`spawn_temporal_processor`] which wires `subscribe()` into a
    /// background task.
    async fn ingest(&self, obs: &Observation);
}

/// Internal per-key state.
#[derive(Debug, Clone)]
struct Series {
    /// Recent samples within the window. `(timestamp, scalar?, payload)`.
    samples: VecDeque<Sample>,
    first_at: SystemTime,
    last_at: SystemTime,
    last_payload: serde_json::Value,
}

#[derive(Debug, Clone)]
struct Sample {
    at: SystemTime,
    scalar: Option<f64>,
}

/// Default in-memory implementation backed by a per-key
/// `VecDeque` window.
pub struct DefaultTemporalProcessor {
    window: Duration,
    state: RwLock<HashMap<(String, Modality), Series>>,
}

impl DefaultTemporalProcessor {
    /// Create with a custom window.
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            state: RwLock::new(HashMap::new()),
        }
    }

    /// Create with [`DEFAULT_TEMPORAL_WINDOW`].
    pub fn with_default_window() -> Self {
        Self::new(DEFAULT_TEMPORAL_WINDOW)
    }

    /// Window duration this processor uses.
    pub fn window(&self) -> Duration {
        self.window
    }

    /// Synchronous push for use by the background ingest task and tests.
    fn push_inner(&self, obs: &Observation) {
        let key = (obs.source.clone(), obs.modality);
        let scalar = extract_scalar(&obs.data);
        let mut state = self.state.write().expect("temporal state poisoned");
        let series = state.entry(key).or_insert_with(|| Series {
            samples: VecDeque::new(),
            first_at: obs.created_at,
            last_at: obs.created_at,
            last_payload: obs.data.clone(),
        });
        series
            .samples
            .push_back(Sample { at: obs.created_at, scalar });
        series.last_at = obs.created_at;
        series.last_payload = obs.data.clone();

        // Evict samples older than `window`.
        let cutoff = obs
            .created_at
            .checked_sub(self.window)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        while let Some(front) = series.samples.front() {
            if front.at < cutoff {
                series.samples.pop_front();
            } else {
                break;
            }
        }
        if let Some(front) = series.samples.front() {
            series.first_at = front.at;
        }
    }
}

#[async_trait]
impl TemporalProcessor for DefaultTemporalProcessor {
    fn snapshot_aggregates(&self) -> HashMap<(String, Modality), Aggregate> {
        let state = self.state.read().expect("temporal state poisoned");
        let now = SystemTime::now();
        let mut out = HashMap::with_capacity(state.len());
        for ((source, modality), series) in state.iter() {
            let count = series.samples.len();
            let mut numeric_count = 0usize;
            let mut min = f64::INFINITY;
            let mut max = f64::NEG_INFINITY;
            let mut sum = 0.0f64;
            for s in &series.samples {
                if let Some(v) = s.scalar {
                    numeric_count += 1;
                    if v < min {
                        min = v;
                    }
                    if v > max {
                        max = v;
                    }
                    sum += v;
                }
            }
            let mut stats = serde_json::Map::new();
            stats.insert("count".into(), serde_json::json!(count));
            stats.insert("last".into(), series.last_payload.clone());
            if numeric_count > 0 {
                stats.insert("numeric_count".into(), serde_json::json!(numeric_count));
                stats.insert("min".into(), serde_json::json!(min));
                stats.insert("max".into(), serde_json::json!(max));
                stats.insert("mean".into(), serde_json::json!(sum / numeric_count as f64));
            }
            out.insert(
                (source.clone(), *modality),
                Aggregate {
                    source: source.clone(),
                    modality: *modality,
                    window: self.window,
                    stats: serde_json::Value::Object(stats),
                    at: now,
                },
            );
        }
        out
    }

    async fn ingest(&self, obs: &Observation) {
        self.push_inner(obs);
    }
}

/// Spawn a background task that subscribes to `hub` and feeds every
/// observation into `processor`. Returns the [`JoinHandle`] so callers
/// can `abort()` on shutdown.
pub fn spawn_temporal_processor(
    hub: Arc<PerceptionStreamHub>,
    processor: Arc<DefaultTemporalProcessor>,
) -> JoinHandle<()> {
    let mut rx = hub.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(obs) => processor.push_inner(&obs),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!("temporal processor lagged, skipped {} observations", skipped);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::debug!("temporal processor: hub closed");
                    break;
                }
            }
        }
    })
}

/// Try to extract a single scalar from an observation payload.
///
/// Recognises:
/// - top-level number,
/// - common scalar keys: `"value"`, `"pct"`, `"rms"`, `"level"`, `"cpu_pct"`,
///   `"mem_pct"`, `"temperature"`, `"celsius"`.
///
/// Returns `None` if no scalar can be unambiguously identified — a
/// multi-field object is recorded only as a count, not an aggregate.
pub fn extract_scalar(v: &serde_json::Value) -> Option<f64> {
    use serde_json::Value;
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Object(map) => {
            const KEYS: &[&str] = &[
                "value",
                "pct",
                "rms",
                "level",
                "cpu_pct",
                "mem_pct",
                "temperature",
                "celsius",
            ];
            for k in KEYS {
                if let Some(Value::Number(n)) = map.get(*k) {
                    return n.as_f64();
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::{Modality, ObservationId};

    fn obs_with(source: &str, modality: Modality, data: serde_json::Value) -> Observation {
        Observation {
            id: ObservationId::new(),
            source: source.to_string(),
            modality,
            timestamp: std::time::Instant::now(),
            created_at: SystemTime::now(),
            confidence: 1.0,
            data,
        }
    }

    #[test]
    fn test_extract_scalar_top_level_number() {
        assert_eq!(extract_scalar(&serde_json::json!(42.5)), Some(42.5));
    }

    #[test]
    fn test_extract_scalar_known_key() {
        assert_eq!(extract_scalar(&serde_json::json!({"value": 7})), Some(7.0));
        assert_eq!(extract_scalar(&serde_json::json!({"cpu_pct": 12.3})), Some(12.3));
    }

    #[test]
    fn test_extract_scalar_unknown_object() {
        assert_eq!(extract_scalar(&serde_json::json!({"foo": 1, "bar": 2})), None);
    }

    #[tokio::test]
    async fn test_ingest_creates_per_key_state() {
        let p = DefaultTemporalProcessor::with_default_window();
        p.ingest(&obs_with("cpu", Modality::System, serde_json::json!({"cpu_pct": 10.0})))
            .await;
        p.ingest(&obs_with("mic", Modality::Audio, serde_json::json!({"rms": 0.5})))
            .await;
        let snap = p.snapshot_aggregates();
        assert_eq!(snap.len(), 2);
        assert!(snap.contains_key(&("cpu".to_string(), Modality::System)));
        assert!(snap.contains_key(&("mic".to_string(), Modality::Audio)));
    }

    #[tokio::test]
    async fn test_aggregate_min_max_mean_for_numeric_series() {
        let p = DefaultTemporalProcessor::new(Duration::from_secs(60));
        for v in [10.0, 20.0, 30.0_f64] {
            p.ingest(&obs_with("cpu", Modality::System, serde_json::json!({"cpu_pct": v})))
                .await;
        }
        let snap = p.snapshot_aggregates();
        let agg = snap.get(&("cpu".to_string(), Modality::System)).unwrap();
        let stats = &agg.stats;
        assert_eq!(stats["count"], 3);
        assert_eq!(stats["min"], 10.0);
        assert_eq!(stats["max"], 30.0);
        assert_eq!(stats["mean"], 20.0);
    }

    #[tokio::test]
    async fn test_aggregate_count_only_for_non_numeric() {
        let p = DefaultTemporalProcessor::new(Duration::from_secs(60));
        p.ingest(&obs_with("fs", Modality::FileSystem, serde_json::json!({"path": "/x/y"})))
            .await;
        let snap = p.snapshot_aggregates();
        let agg = snap.get(&("fs".to_string(), Modality::FileSystem)).unwrap();
        let stats = &agg.stats;
        assert_eq!(stats["count"], 1);
        assert!(stats.get("mean").is_none());
        assert_eq!(stats["last"]["path"], "/x/y");
    }

    #[tokio::test]
    async fn test_window_evicts_old_samples() {
        let p = DefaultTemporalProcessor::new(Duration::from_millis(100));
        // Push a "stale" sample with an old created_at.
        let mut old_obs = obs_with("cpu", Modality::System, serde_json::json!(1.0));
        old_obs.created_at = SystemTime::now() - Duration::from_secs(10);
        p.ingest(&old_obs).await;

        // Push a fresh sample — eviction reference is the new sample's `created_at`.
        let fresh = obs_with("cpu", Modality::System, serde_json::json!(2.0));
        p.ingest(&fresh).await;

        let snap = p.snapshot_aggregates();
        let agg = snap.get(&("cpu".to_string(), Modality::System)).unwrap();
        // Only the fresh sample should remain.
        assert_eq!(agg.stats["count"], 1);
        assert_eq!(agg.stats["last"], 2.0);
    }

    #[tokio::test]
    async fn test_spawn_processor_subscribes_to_hub() {
        let hub = Arc::new(PerceptionStreamHub::new(64));
        let proc = Arc::new(DefaultTemporalProcessor::with_default_window());
        let _h = spawn_temporal_processor(hub.clone(), proc.clone());

        // Use a streaming MockPerceptionSource to push observations through hub.
        use crate::perception::mock::MockPerceptionSource;
        let (mock, tx) = MockPerceptionSource::new("streamer").with_streaming(64);
        hub.attach_source(
            "streamer",
            Arc::new(mock) as Arc<dyn crate::perception::PerceptionSource>,
        )
        .await;

        let obs = obs_with("streamer", Modality::System, serde_json::json!({"value": 7.0}));
        tx.send(obs).unwrap();

        // Allow the spawned task to run.
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let snap = proc.snapshot_aggregates();
            if !snap.is_empty() {
                let agg = snap
                    .get(&("streamer".to_string(), Modality::System))
                    .unwrap();
                assert_eq!(agg.stats["count"], 1);
                assert_eq!(agg.stats["mean"], 7.0);
                return;
            }
        }
        panic!("temporal processor did not receive observation in time");
    }
}
