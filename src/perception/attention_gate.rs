//! Per-agent attention gate.
//!
//! [`AttentionGate`] is the **first per-agent stage** downstream of
//! [`super::DerivedStreamHub`]. It cheaply rejects events the agent
//! doesn't care about so they never reach the heavier
//! [`super::SalienceFilter`] / summariser stages.
//!
//! # Pass rules
//!
//! 1. [`Event::Anomaly`] always passes (bypass channel).
//! 2. If `focus.modalities` is `Some(set)` and the event carries a
//!    modality, that modality must be in `set`.
//! 3. If `focus.sources` is `Some(set)` and the event carries a source,
//!    that source must be in `set`.
//! 4. `focus.freq_budget[modality]` enforces a minimum spacing between
//!    consecutive admits of that modality (1/Hz seconds).
//!
//! Events without a modality / source (e.g. [`Event::Entity`]) are not
//! filtered by the corresponding whitelist — they span sources by
//! design.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::perception::{Event, Focus, Modality};

/// Per-agent admission filter for [`Event`]s.
pub struct AttentionGate {
    focus: Focus,
    /// Last admitted timestamp per modality (for freq-budget enforcement).
    last_admit: HashMap<Modality, Instant>,
}

impl AttentionGate {
    /// Create a new gate with the given initial focus.
    pub fn new(focus: Focus) -> Self {
        Self {
            focus,
            last_admit: HashMap::new(),
        }
    }

    /// Borrow the current focus.
    pub fn focus(&self) -> &Focus {
        &self.focus
    }

    /// Replace the focus.
    ///
    /// Per the pipeline spec: whitelist is replaced, freq_budget timers
    /// reset (the next event for any modality is admitted by the
    /// rate-limit step regardless of how recently the modality was
    /// last admitted).
    pub fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
        self.last_admit.clear();
    }

    /// Decide whether `event` is admitted.
    ///
    /// Side effect: when an event passes the freq-budget check, its
    /// timestamp is recorded so the next event for the same modality
    /// is rate-limited.
    pub fn admit(&mut self, event: &Event) -> bool {
        // 1. Anomaly bypass.
        if event.is_anomaly() {
            return true;
        }

        // 2. Modality whitelist (only enforced if the event has a modality).
        if let (Some(allowed), Some(m)) = (&self.focus.modalities, event.modality()) {
            if !allowed.contains(&m) {
                return false;
            }
        }

        // 3. Source whitelist (only enforced if the event has a source).
        if let (Some(allowed), Some(s)) = (&self.focus.sources, event.source()) {
            if !allowed.contains(s) {
                return false;
            }
        }

        // 4. Freq budget — minimum-spacing rate limit per modality.
        if let Some(m) = event.modality() {
            if let Some(&hz) = self.focus.freq_budget.get(&m) {
                if hz > 0.0 {
                    let interval = Duration::from_secs_f32(1.0 / hz);
                    let now = Instant::now();
                    if let Some(&last) = self.last_admit.get(&m) {
                        if now.duration_since(last) < interval {
                            return false;
                        }
                    }
                    self.last_admit.insert(m, now);
                }
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::{AnomalyKind, Modality};
    use std::time::SystemTime;

    fn change(source: &str, modality: Modality) -> Event {
        Event::Change {
            source: source.to_string(),
            modality,
            from: serde_json::json!(0),
            to: serde_json::json!(1),
            at: SystemTime::now(),
        }
    }

    fn anomaly(source: &str) -> Event {
        Event::Anomaly {
            source: source.to_string(),
            reason: AnomalyKind::SourceFault,
            severity: 200,
            at: SystemTime::now(),
        }
    }

    #[test]
    fn test_default_focus_admits_everything() {
        let mut gate = AttentionGate::new(Focus::default());
        assert!(gate.admit(&change("cpu", Modality::System)));
        assert!(gate.admit(&change("mic", Modality::Audio)));
        assert!(gate.admit(&change("fs", Modality::FileSystem)));
    }

    #[test]
    fn test_modality_whitelist_rejects_mismatch() {
        let focus = Focus::default().with_modalities([Modality::System]);
        let mut gate = AttentionGate::new(focus);
        assert!(gate.admit(&change("cpu", Modality::System)));
        assert!(!gate.admit(&change("mic", Modality::Audio)));
    }

    #[test]
    fn test_source_whitelist_rejects_mismatch() {
        let focus = Focus::default().with_sources(["cpu"]);
        let mut gate = AttentionGate::new(focus);
        assert!(gate.admit(&change("cpu", Modality::System)));
        assert!(!gate.admit(&change("mic", Modality::Audio)));
    }

    #[test]
    fn test_anomaly_bypasses_all_filters() {
        // Tight focus that would normally block anything other than `cpu`/`System`.
        let focus = Focus::default()
            .with_modalities([Modality::System])
            .with_sources(["cpu"]);
        let mut gate = AttentionGate::new(focus);
        // Anomaly from a different source / no specific modality still passes.
        assert!(gate.admit(&anomaly("mic")));
    }

    #[test]
    fn test_freq_budget_enforces_minimum_spacing() {
        let focus = Focus::default().with_freq_budget(Modality::System, 100.0); // 10ms interval
        let mut gate = AttentionGate::new(focus);
        // First event always admitted.
        assert!(gate.admit(&change("cpu", Modality::System)));
        // Immediate burst rejected.
        assert!(!gate.admit(&change("cpu", Modality::System)));
        // After spacing, admitted again.
        std::thread::sleep(Duration::from_millis(15));
        assert!(gate.admit(&change("cpu", Modality::System)));
    }

    #[test]
    fn test_freq_budget_is_per_modality() {
        let focus = Focus::default().with_freq_budget(Modality::System, 1.0); // 1 Hz
        let mut gate = AttentionGate::new(focus);
        assert!(gate.admit(&change("cpu", Modality::System)));
        // Different modality unaffected.
        assert!(gate.admit(&change("mic", Modality::Audio)));
        // Same modality blocked.
        assert!(!gate.admit(&change("cpu", Modality::System)));
    }

    #[test]
    fn test_set_focus_resets_freq_state() {
        let focus = Focus::default().with_freq_budget(Modality::System, 1.0); // 1 Hz
        let mut gate = AttentionGate::new(focus.clone());
        assert!(gate.admit(&change("cpu", Modality::System)));
        assert!(!gate.admit(&change("cpu", Modality::System)));

        // Re-applying focus clears freq state — next admit succeeds.
        gate.set_focus(focus);
        assert!(gate.admit(&change("cpu", Modality::System)));
    }

    #[test]
    fn test_entity_event_bypasses_source_whitelist() {
        use crate::perception::FusedEntity;
        use std::collections::HashMap;
        let focus = Focus::default().with_sources(["cpu"]);
        let mut gate = AttentionGate::new(focus);
        let now_inst = Instant::now();
        let ev = Event::Entity {
            entity: FusedEntity {
                id: "e1".into(),
                label: "test".into(),
                created_at: now_inst,
                updated_at: now_inst,
                confidence: 1.0,
                modalities: vec![],
                observation_ids: vec![],
                properties: HashMap::new(),
                correlation_key: "k".into(),
            },
            at: SystemTime::now(),
        };
        // Entity has no `source()` — admitted despite the source whitelist.
        assert!(gate.admit(&ev));
    }
}
