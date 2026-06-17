//! Per-agent perception focus configuration.
//!
//! [`Focus`] declares **what** an agent currently cares about — it drives
//! [`super::AttentionGate`] (which observations to admit) and
//! [`super::SalienceFilter`] (how aggressively to suppress noise).
//!
//! Focus is **per-agent state**: every agent owns its own `Focus`, and
//! changing it is the only way for the agent to redirect perception
//! resources without restarting the pipeline.
//!
//! # Example
//!
//! ```ignore
//! let focus = Focus::default()
//!     .with_modalities([Modality::System, Modality::FileSystem])
//!     .with_freq_budget(Modality::System, 5.0)        // ≤ 5 Hz
//!     .with_delta_threshold(Modality::System, 5.0);   // ≥ 5% change
//! adapter.focus(focus);
//! ```

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::Serialize;

use crate::perception::Modality;

/// Default minimum-confidence cutoff (suppress noisy detector output).
pub const DEFAULT_MIN_CONFIDENCE: f32 = 0.0;

/// Default dedup window — collapse identical observations within 500 ms.
pub const DEFAULT_DEDUP_WINDOW: Duration = Duration::from_millis(500);

/// Default `baseline_max_age` for diff baselines (see
/// [`SalienceConfig::baseline_max_age`]).
pub const DEFAULT_BASELINE_MAX_AGE: Duration = Duration::from_secs(10);

/// Per-agent perception focus.
///
/// `None` for `modalities`/`sources` means "everything"; explicit sets
/// restrict admission. `freq_budget` is per-(source, modality) — empty
/// map means no rate limiting.
#[derive(Debug, Clone, Serialize)]
pub struct Focus {
    /// Modality whitelist; `None` = admit all modalities.
    pub modalities: Option<HashSet<Modality>>,
    /// Source name whitelist; `None` = admit all sources.
    pub sources: Option<HashSet<String>>,
    /// Per-modality frequency cap in Hz. Missing keys = unlimited.
    pub freq_budget: HashMap<Modality, f32>,
    /// Salience filter parameters.
    pub salience: SalienceConfig,
}

impl Default for Focus {
    fn default() -> Self {
        Self {
            modalities: None,
            sources: None,
            freq_budget: HashMap::new(),
            salience: SalienceConfig::default(),
        }
    }
}

impl Focus {
    /// Restrict to the given modalities.
    pub fn with_modalities<I: IntoIterator<Item = Modality>>(mut self, modalities: I) -> Self {
        self.modalities = Some(modalities.into_iter().collect());
        self
    }

    /// Restrict to the given source names.
    pub fn with_sources<I, S>(mut self, sources: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.sources = Some(sources.into_iter().map(Into::into).collect());
        self
    }

    /// Set a frequency cap for one modality (Hz).
    pub fn with_freq_budget(mut self, modality: Modality, hz: f32) -> Self {
        self.freq_budget.insert(modality, hz);
        self
    }

    /// Set the salience-filter delta threshold for one modality.
    pub fn with_delta_threshold(mut self, modality: Modality, threshold: f32) -> Self {
        self.salience.delta_threshold.insert(modality, threshold);
        self
    }

    /// Whether `modality` is in this focus.
    pub fn admits_modality(&self, modality: Modality) -> bool {
        self.modalities
            .as_ref()
            .map(|s| s.contains(&modality))
            .unwrap_or(true)
    }

    /// Whether `source` is in this focus.
    pub fn admits_source(&self, source: &str) -> bool {
        self.sources
            .as_ref()
            .map(|s| s.contains(source))
            .unwrap_or(true)
    }
}

/// Salience-filter knobs. All fields are read by
/// [`super::SalienceFilter`] each time it evaluates an observation.
#[derive(Debug, Clone, Serialize)]
pub struct SalienceConfig {
    /// Per-modality delta threshold for [`super::Event::Change`]. The
    /// interpretation depends on modality:
    ///
    /// - Numeric scalars: relative change in percent (e.g. `5.0` = 5%).
    /// - Compound payloads: implementation-defined; default is a JSON
    ///   diff distance.
    pub delta_threshold: HashMap<Modality, f32>,
    /// Window inside which identical observations are deduplicated.
    pub dedup_window: Duration,
    /// Observations with `confidence < min_confidence` are suppressed
    /// (anomalies always pass regardless).
    pub min_confidence: f32,
    /// Diff baselines older than this are discarded — the next sample
    /// becomes the new baseline (no Change event emitted). Prevents
    /// stale baselines from generating spurious Change events after a
    /// long focus gap.
    pub baseline_max_age: Duration,
}

impl Default for SalienceConfig {
    fn default() -> Self {
        Self {
            delta_threshold: HashMap::new(),
            dedup_window: DEFAULT_DEDUP_WINDOW,
            min_confidence: DEFAULT_MIN_CONFIDENCE,
            baseline_max_age: DEFAULT_BASELINE_MAX_AGE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_admits_everything() {
        let f = Focus::default();
        assert!(f.admits_modality(Modality::System));
        assert!(f.admits_modality(Modality::Audio));
        assert!(f.admits_source("anything"));
    }

    #[test]
    fn test_with_modalities_restricts() {
        let f = Focus::default().with_modalities([Modality::System, Modality::FileSystem]);
        assert!(f.admits_modality(Modality::System));
        assert!(f.admits_modality(Modality::FileSystem));
        assert!(!f.admits_modality(Modality::Audio));
    }

    #[test]
    fn test_with_sources_restricts() {
        let f = Focus::default().with_sources(["cpu", "fs"]);
        assert!(f.admits_source("cpu"));
        assert!(!f.admits_source("mic"));
    }

    #[test]
    fn test_freq_budget_round_trip() {
        let f = Focus::default().with_freq_budget(Modality::Audio, 10.0);
        assert_eq!(f.freq_budget.get(&Modality::Audio).copied(), Some(10.0));
    }

    #[test]
    fn test_salience_default_window_and_age() {
        let s = SalienceConfig::default();
        assert_eq!(s.dedup_window, DEFAULT_DEDUP_WINDOW);
        assert_eq!(s.baseline_max_age, DEFAULT_BASELINE_MAX_AGE);
    }

    #[test]
    fn test_focus_serializes() {
        let f = Focus::default()
            .with_modalities([Modality::System])
            .with_freq_budget(Modality::System, 1.0);
        let v = serde_json::to_value(&f).unwrap();
        assert!(v.get("modalities").is_some());
        assert!(v.get("freq_budget").is_some());
    }
}
