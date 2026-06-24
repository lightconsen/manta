//! Per-agent salience filter.
//!
//! Produces [`Event::Change`] from raw [`Observation`]s when the value
//! drifts further than [`SalienceConfig::delta_threshold`] from the
//! current diff baseline. Also enforces
//! [`SalienceConfig::min_confidence`] and dedups identical-payload
//! observations within [`SalienceConfig::dedup_window`].
//!
//! # State ownership
//!
//! `SalienceFilter` is **per-agent** state. Two agents looking at the
//! same `(source, modality)` may have different thresholds and
//! therefore disagree on whether a given observation is salient — the
//! shared [`super::TemporalProcessor`] does not produce Change events
//! for this reason.
//!
//! # Baseline staleness
//!
//! After `baseline_max_age` of inactivity for a given key, the next
//! observation becomes a *fresh baseline* (no Change emitted). This
//! prevents a stale baseline from triggering spurious Change events
//! when the agent re-focuses after a long gap.

use std::collections::HashMap;
use std::time::Instant;

use crate::perception::{Event, Modality, Observation, SalienceConfig};

/// Per-key diff baseline.
#[derive(Debug, Clone)]
struct Baseline {
    payload: serde_json::Value,
    scalar: Option<f64>,
    at: Instant,
}

/// Per-key dedup record.
#[derive(Debug, Clone)]
struct DedupRecord {
    signature: String,
    at: Instant,
}

/// Per-agent salience filter.
pub struct SalienceFilter {
    config: SalienceConfig,
    baselines: HashMap<(String, Modality), Baseline>,
    recent: HashMap<(String, Modality), DedupRecord>,
}

impl SalienceFilter {
    /// Create with the given configuration.
    pub fn new(config: SalienceConfig) -> Self {
        Self {
            config,
            baselines: HashMap::new(),
            recent: HashMap::new(),
        }
    }

    /// Borrow the current configuration.
    pub fn config(&self) -> &SalienceConfig {
        &self.config
    }

    /// Replace the configuration.
    ///
    /// Per the pipeline spec on focus change: the dedup cache is
    /// cleared, but baselines are *retained* (so we don't lose the
    /// reference value just because the user adjusted thresholds).
    /// `baseline_max_age` will lazily evict any that have aged out.
    pub fn set_config(&mut self, config: SalienceConfig) {
        self.config = config;
        self.recent.clear();
    }

    /// Drop all dedup records (e.g. after focus change).
    pub fn clear_dedup(&mut self) {
        self.recent.clear();
    }

    /// Drop all baselines (e.g. on full reset).
    pub fn reset(&mut self) {
        self.baselines.clear();
        self.recent.clear();
    }

    /// Evaluate an observation; emit `Event::Change` iff:
    ///
    /// 1. confidence ≥ `min_confidence`,
    /// 2. payload differs from the dedup signature (or dedup window expired),
    /// 3. a baseline exists *and* is fresher than `baseline_max_age`,
    /// 4. the modality has an entry in `delta_threshold`,
    /// 5. the relative scalar change (or payload inequality) exceeds the
    ///    threshold.
    ///
    /// All of (1–5) failing returns `None`. When (3) is the failing
    /// reason, the new observation is installed as the fresh baseline
    /// for next time.
    pub fn evaluate(&mut self, obs: &Observation) -> Option<Event> {
        // (1) confidence floor.
        if obs.confidence < self.config.min_confidence {
            return None;
        }

        let key = (obs.source.clone(), obs.modality);
        let now = Instant::now();
        let signature = obs.data.to_string();

        // (2) dedup.
        if let Some(rec) = self.recent.get(&key) {
            if rec.signature == signature && now.duration_since(rec.at) < self.config.dedup_window {
                return None;
            }
        }

        let new_scalar = super::temporal_processor::extract_scalar(&obs.data);

        // (3) baseline freshness.
        let baseline_stale = self
            .baselines
            .get(&key)
            .map(|b| now.duration_since(b.at) > self.config.baseline_max_age)
            .unwrap_or(true);

        if baseline_stale {
            self.baselines.insert(
                key.clone(),
                Baseline {
                    payload: obs.data.clone(),
                    scalar: new_scalar,
                    at: now,
                },
            );
            self.recent.insert(key, DedupRecord { signature, at: now });
            return None;
        }

        // (4) modality opt-in.
        let threshold = match self.config.delta_threshold.get(&obs.modality) {
            Some(&t) => t,
            None => return None,
        };

        // (5) delta vs threshold.
        #[allow(clippy::expect_used)] // baseline presence ensured by step (4) above
        let baseline = self.baselines.get(&key).expect("baseline checked above");
        let above = match (baseline.scalar, new_scalar) {
            (Some(old), Some(new)) => {
                let denom = old.abs().max(f64::EPSILON);
                let relative_pct = ((new - old).abs() / denom * 100.0) as f32;
                relative_pct >= threshold
            }
            _ => baseline.payload != obs.data,
        };
        if !above {
            return None;
        }

        // Emit + update baseline + dedup.
        let from = baseline.payload.clone();
        self.baselines.insert(
            key.clone(),
            Baseline {
                payload: obs.data.clone(),
                scalar: new_scalar,
                at: now,
            },
        );
        self.recent.insert(key, DedupRecord { signature, at: now });

        Some(Event::Change {
            source: obs.source.clone(),
            modality: obs.modality,
            from,
            to: obs.data.clone(),
            at: obs.created_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::perception::{Modality, Observation, ObservationId};

    fn obs(source: &str, modality: Modality, data: serde_json::Value, conf: f32) -> Observation {
        Observation {
            id: ObservationId::new(),
            source: source.to_string(),
            modality,
            timestamp: std::time::Instant::now(),
            created_at: SystemTime::now(),
            confidence: conf,
            data,
        }
    }

    fn cfg_with_threshold(modality: Modality, pct: f32) -> SalienceConfig {
        let mut c = SalienceConfig::default();
        c.delta_threshold.insert(modality, pct);
        c
    }

    #[test]
    fn test_first_observation_installs_baseline_no_event() {
        let mut f = SalienceFilter::new(cfg_with_threshold(Modality::System, 5.0));
        let ev =
            f.evaluate(&obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 10.0}), 1.0));
        assert!(ev.is_none(), "first sample is baseline-only");
    }

    #[test]
    fn test_below_threshold_suppressed() {
        let mut f = SalienceFilter::new(cfg_with_threshold(Modality::System, 50.0));
        f.evaluate(&obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 10.0}), 1.0));
        // 10 → 11 = 10% relative change, below threshold (50).
        let ev =
            f.evaluate(&obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 11.0}), 1.0));
        assert!(ev.is_none());
    }

    #[test]
    fn test_above_threshold_emits_change() {
        let mut f = SalienceFilter::new(cfg_with_threshold(Modality::System, 5.0));
        f.evaluate(&obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 10.0}), 1.0));
        // 10 → 20 = 100% change, well above 5%.
        let ev = f
            .evaluate(&obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 20.0}), 1.0))
            .expect("should emit Change");
        match ev {
            Event::Change { source, modality, from, to, .. } => {
                assert_eq!(source, "cpu");
                assert_eq!(modality, Modality::System);
                assert_eq!(from["cpu_pct"], 10.0);
                assert_eq!(to["cpu_pct"], 20.0);
            }
            _ => panic!("expected Change"),
        }
    }

    #[test]
    fn test_min_confidence_filters_low_conf() {
        let mut c = cfg_with_threshold(Modality::System, 5.0);
        c.min_confidence = 0.5;
        let mut f = SalienceFilter::new(c);
        let ev = f.evaluate(&obs(
            "cpu",
            Modality::System,
            serde_json::json!({"cpu_pct": 10.0}),
            0.2, // below cutoff
        ));
        assert!(ev.is_none());
        // Even higher conf should now establish baseline (no event yet).
        let ev =
            f.evaluate(&obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 100.0}), 0.9));
        assert!(ev.is_none());
    }

    #[test]
    fn test_modality_without_threshold_emits_nothing() {
        // No delta_threshold registered for Audio — never emit.
        let f_default = SalienceConfig::default();
        let mut f = SalienceFilter::new(f_default);
        // Establish baseline.
        f.evaluate(&obs("mic", Modality::Audio, serde_json::json!({"rms": 0.1}), 1.0));
        // Big change but no opt-in → still None.
        let ev = f.evaluate(&obs("mic", Modality::Audio, serde_json::json!({"rms": 100.0}), 1.0));
        assert!(ev.is_none());
    }

    #[test]
    fn test_dedup_suppresses_repeated_payload() {
        let mut c = cfg_with_threshold(Modality::System, 0.1);
        c.dedup_window = Duration::from_secs(5);
        let mut f = SalienceFilter::new(c);
        let payload = serde_json::json!({"cpu_pct": 10.0});
        // First call: baseline.
        f.evaluate(&obs("cpu", Modality::System, payload.clone(), 1.0));
        // Second call with identical payload: dedup'd.
        let ev = f.evaluate(&obs("cpu", Modality::System, payload.clone(), 1.0));
        assert!(ev.is_none());
    }

    #[test]
    fn test_set_config_clears_dedup_keeps_baseline() {
        let mut f = SalienceFilter::new(cfg_with_threshold(Modality::System, 5.0));
        f.evaluate(&obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 10.0}), 1.0));
        assert!(!f.baselines.is_empty());

        // Re-config: dedup cleared, baselines retained.
        f.set_config(cfg_with_threshold(Modality::System, 1.0));
        assert!(f.recent.is_empty());
        assert!(!f.baselines.is_empty());
    }

    #[test]
    fn test_reset_clears_everything() {
        let mut f = SalienceFilter::new(cfg_with_threshold(Modality::System, 5.0));
        f.evaluate(&obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 10.0}), 1.0));
        f.reset();
        assert!(f.baselines.is_empty());
        assert!(f.recent.is_empty());
    }

    #[test]
    fn test_baseline_max_age_expiry_resets_to_fresh() {
        // After baseline_max_age elapses, the next obs must become a
        // fresh baseline (no Change emitted) — guards against stale
        // baselines triggering spurious Changes when the agent re-focuses
        // after a long gap.
        let mut c = cfg_with_threshold(Modality::System, 5.0);
        c.baseline_max_age = Duration::from_millis(50);
        let mut f = SalienceFilter::new(c);
        // Install baseline.
        f.evaluate(&obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 10.0}), 1.0));
        std::thread::sleep(Duration::from_millis(80));
        // 10 → 90 = 800% delta, well above the 5% threshold — but the
        // baseline is now stale, so the filter installs a new baseline
        // and emits no event.
        let ev =
            f.evaluate(&obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 90.0}), 1.0));
        assert!(ev.is_none(), "stale-baseline path should suppress event");
    }

    #[test]
    fn test_non_numeric_payload_change_uses_inequality_path() {
        // When neither the baseline nor the new observation produces a
        // scalar via `extract_scalar`, the filter falls through to the
        // payload-inequality branch (`baseline.payload != obs.data`).
        let c = cfg_with_threshold(Modality::Other, 0.1);
        let mut f = SalienceFilter::new(c);
        // Baseline with non-numeric payload.
        f.evaluate(&obs("plug", Modality::Other, serde_json::json!({"state": "on"}), 1.0));
        // Different non-numeric payload — inequality path emits Change.
        let ev = f
            .evaluate(&obs("plug", Modality::Other, serde_json::json!({"state": "off"}), 1.0))
            .expect("inequality path should emit Change");
        match ev {
            Event::Change { source, modality, from, to, .. } => {
                assert_eq!(source, "plug");
                assert_eq!(modality, Modality::Other);
                assert_eq!(from["state"], "on");
                assert_eq!(to["state"], "off");
            }
            other => panic!("expected Change, got {other:?}"),
        }
    }

    #[test]
    fn test_dedup_window_expiry_lets_repeat_through_as_baseline_replace() {
        // Identical payloads inside dedup_window are suppressed. After
        // the window expires, the same payload is no longer dedup'd —
        // it goes through the baseline-fresh path and (since the value
        // didn't actually change) emits no Change either.
        let mut c = cfg_with_threshold(Modality::System, 0.1);
        c.dedup_window = Duration::from_millis(50);
        let mut f = SalienceFilter::new(c);
        let payload = serde_json::json!({"cpu_pct": 10.0});
        f.evaluate(&obs("cpu", Modality::System, payload.clone(), 1.0));
        // Within the window — dedup'd.
        assert!(f
            .evaluate(&obs("cpu", Modality::System, payload.clone(), 1.0))
            .is_none());
        std::thread::sleep(Duration::from_millis(80));
        // After expiry, the dedup record is stale so the dedup guard
        // doesn't trip — but value is identical so no Change emitted.
        assert!(f
            .evaluate(&obs("cpu", Modality::System, payload, 1.0))
            .is_none());
    }
}
