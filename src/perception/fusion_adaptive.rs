//! Adaptive fusion parameter tuning.
//!
//! [`SensorNoiseTracker`] watches observation timing across modalities and
//! recommends a [`FusionConfig::temporal_window_ms`] based on observed
//! inter-arrival jitter. When sensors are noisy or asynchronous, the
//! window grows; when they arrive close together it tightens.
//!
//! Recommended deployment: spawn a background task that, every N seconds,
//! pulls the latest observations from `PerceptionRegistry::all_observations()`,
//! feeds them to `tracker.observe()`, then calls
//! `engine.update_config(|c| c.temporal_window_ms = tracker.recommend_window_ms())`.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::perception::{Modality, Observation};

/// Sample budget — how many recent inter-arrival samples to keep per modality pair.
const SAMPLE_BUDGET: usize = 256;

/// Minimum window the tracker will ever recommend.
const MIN_WINDOW_MS: u64 = 50;

/// Maximum window the tracker will ever recommend.
const MAX_WINDOW_MS: u64 = 5_000;

/// Multiplier applied to p95 inter-arrival when recommending window.
const SAFETY_FACTOR: f64 = 1.5;

/// Tracks inter-modality timing jitter and recommends fusion windows.
#[derive(Debug, Default)]
pub struct SensorNoiseTracker {
    /// For each modality pair (a, b) where a < b in Debug-string order, keep
    /// recent inter-arrival samples (in milliseconds).
    samples: HashMap<(Modality, Modality), VecDeque<u64>>,
    /// Last observation timestamp seen per modality (for next-pair compute).
    last_per_modality: HashMap<Modality, Instant>,
}

impl SensorNoiseTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a batch of observations. The order in `obs` does not matter —
    /// they are sorted internally before computing inter-arrival.
    pub fn observe(&mut self, obs: &[Observation]) {
        if obs.len() < 2 {
            return;
        }
        let mut sorted: Vec<&Observation> = obs.iter().collect();
        sorted.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        // For each adjacent pair of distinct modalities, record |Δt|.
        for win in sorted.windows(2) {
            let (a, b) = (win[0], win[1]);
            if a.modality == b.modality {
                continue;
            }
            let dt_ms = b.timestamp.saturating_duration_since(a.timestamp).as_millis() as u64;
            let key = pair_key(a.modality, b.modality);
            let bucket = self.samples.entry(key).or_default();
            if bucket.len() == SAMPLE_BUDGET {
                bucket.pop_front();
            }
            bucket.push_back(dt_ms);
        }

        // Update last-seen per modality.
        for o in &sorted {
            self.last_per_modality.insert(o.modality, o.timestamp);
        }
    }

    /// Compute the per-pair p95 of inter-arrival samples and pick the max.
    /// Returns `None` if no samples have been recorded yet.
    pub fn p95_max_ms(&self) -> Option<u64> {
        let mut max_p95: Option<u64> = None;
        for bucket in self.samples.values() {
            if bucket.is_empty() {
                continue;
            }
            let p = percentile_u64(bucket, 0.95);
            max_p95 = Some(match max_p95 {
                Some(cur) => cur.max(p),
                None => p,
            });
        }
        max_p95
    }

    /// Recommend a `temporal_window_ms` based on observed jitter.
    /// Falls back to a sensible default if no samples are available.
    pub fn recommend_window_ms(&self) -> u64 {
        let p95 = self.p95_max_ms().unwrap_or(0);
        let scaled = (p95 as f64 * SAFETY_FACTOR).round() as u64;
        scaled.clamp(MIN_WINDOW_MS, MAX_WINDOW_MS)
    }

    /// Recommend a new [`FusionConfig`](crate::perception::FusionConfig) by
    /// merging the recommended window onto a baseline config.
    pub fn recommend_config(
        &self,
        baseline: &crate::perception::FusionConfig,
    ) -> crate::perception::FusionConfig {
        crate::perception::FusionConfig {
            temporal_window_ms: self.recommend_window_ms(),
            min_confidence: baseline.min_confidence,
        }
    }

    /// Total number of samples currently stored.
    pub fn sample_count(&self) -> usize {
        self.samples.values().map(|v| v.len()).sum()
    }

    /// Drop all samples — useful on hotplug events that change the sensor mix.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.last_per_modality.clear();
    }
}

/// Symmetric pair key — modality order is normalised by Debug string.
fn pair_key(a: Modality, b: Modality) -> (Modality, Modality) {
    if format!("{a:?}") <= format!("{b:?}") {
        (a, b)
    } else {
        (b, a)
    }
}

/// Compute an approximate percentile from a deque of u64 samples.
fn percentile_u64(data: &VecDeque<u64>, p: f64) -> u64 {
    if data.is_empty() {
        return 0;
    }
    let mut v: Vec<u64> = data.iter().copied().collect();
    v.sort_unstable();
    let idx = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[idx.min(v.len() - 1)]
}

/// Spawn a background task that periodically retunes the [`FusionEngine`]'s
/// `temporal_window_ms` based on observations from a [`PerceptionRegistry`].
///
/// The task lives until the registry / engine are dropped or the returned
/// [`tokio::task::JoinHandle`] is aborted.
pub fn spawn_adaptive_fusion_loop(
    registry: std::sync::Arc<crate::perception::PerceptionRegistry>,
    engine: crate::perception::FusionEngine,
    tune_interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tracker = SensorNoiseTracker::new();
        let mut ticker = tokio::time::interval(tune_interval);
        // Skip the first immediate tick.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let obs = registry.all_observations().await;
            tracker.observe(&obs);
            let baseline = engine.config().await;
            let new_window = tracker.recommend_window_ms();
            if new_window != baseline.temporal_window_ms {
                tracing::debug!(
                    "adaptive fusion: temporal_window_ms {} → {} (samples={})",
                    baseline.temporal_window_ms,
                    new_window,
                    tracker.sample_count(),
                );
                engine
                    .update_config(|c| c.temporal_window_ms = new_window)
                    .await;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::ObservationId;

    fn obs_at(modality: Modality, ts: Instant) -> Observation {
        Observation {
            id: ObservationId::new(),
            source: format!("{modality:?}"),
            modality,
            timestamp: ts,
            created_at: std::time::SystemTime::now(),
            confidence: 1.0,
            data: serde_json::Value::Null,
        }
    }

    #[test]
    fn test_recommend_window_with_no_samples_is_min() {
        let t = SensorNoiseTracker::new();
        assert_eq!(t.recommend_window_ms(), MIN_WINDOW_MS);
    }

    #[test]
    fn test_recommend_window_grows_with_jitter() {
        let mut t = SensorNoiseTracker::new();
        let base = Instant::now();
        // Two modalities arriving 200ms apart consistently.
        let mut obs = Vec::new();
        for i in 0..32 {
            obs.push(obs_at(Modality::Rgb, base + Duration::from_millis(i * 200)));
            obs.push(obs_at(
                Modality::Audio,
                base + Duration::from_millis(i * 200 + 200),
            ));
        }
        t.observe(&obs);
        let rec = t.recommend_window_ms();
        // 200ms * 1.5 ≈ 300ms (within float rounding)
        assert!(
            (250..=400).contains(&rec),
            "expected ~300ms window, got {rec}"
        );
    }

    #[test]
    fn test_pair_key_is_symmetric() {
        assert_eq!(
            pair_key(Modality::Rgb, Modality::Audio),
            pair_key(Modality::Audio, Modality::Rgb)
        );
    }

    #[test]
    fn test_sample_budget_bounded() {
        let mut t = SensorNoiseTracker::new();
        let base = Instant::now();
        let mut obs = Vec::new();
        for i in 0..(SAMPLE_BUDGET as u64 * 4) {
            obs.push(obs_at(Modality::Rgb, base + Duration::from_millis(i * 10)));
            obs.push(obs_at(
                Modality::Audio,
                base + Duration::from_millis(i * 10 + 5),
            ));
        }
        t.observe(&obs);
        // total samples per pair should be ≤ SAMPLE_BUDGET
        for bucket in t.samples.values() {
            assert!(bucket.len() <= SAMPLE_BUDGET);
        }
    }

    #[test]
    fn test_recommend_window_clamps_to_max() {
        let mut t = SensorNoiseTracker::new();
        let base = Instant::now();
        // Massive 10s gap → should clamp to MAX_WINDOW_MS, not produce 15s.
        let obs = vec![
            obs_at(Modality::Rgb, base),
            obs_at(Modality::Audio, base + Duration::from_secs(10)),
        ];
        t.observe(&obs);
        assert_eq!(t.recommend_window_ms(), MAX_WINDOW_MS);
    }

    #[test]
    fn test_reset_clears_samples() {
        let mut t = SensorNoiseTracker::new();
        let base = Instant::now();
        t.observe(&[
            obs_at(Modality::Rgb, base),
            obs_at(Modality::Audio, base + Duration::from_millis(100)),
        ]);
        assert!(t.sample_count() > 0);
        t.reset();
        assert_eq!(t.sample_count(), 0);
    }
}
