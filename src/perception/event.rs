//! Derived perception events — the post-pipeline output formats.
//!
//! Once observations leave the [`PerceptionStreamHub`](super::stream::PerceptionStreamHub)
//! and pass through the temporal/fusion stages, they are no longer raw
//! samples but rather **events**: discrete things that happened in the
//! world. Events are the unit of consumption for downstream agents.
//!
//! Three flavours coexist:
//!
//! - [`Event::Change`]   — a tracked numeric/state value moved enough to matter.
//! - [`Event::Discrete`] — a one-shot occurrence (file write, key press, …).
//! - [`Event::Anomaly`]  — a high-priority signal that bypasses normal gating.
//! - [`Event::Entity`]   — output of cross-modal fusion (a [`FusedEntity`]).
//!
//! [`Aggregate`] is the companion type for sliding-window summaries (mean,
//! min, max, …) over a `(source, modality)` pair. Aggregates are read via
//! [`super::Snapshot`] rather than streamed.

use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::perception::{FusedEntity, Modality};

/// A derived perception event — produced by the temporal/fusion pipeline,
/// consumed by per-agent adapters.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A tracked state/numeric value changed by more than the salience
    /// threshold. The most common event for system metrics.
    Change {
        /// Source name (matches [`super::Observation::source`]).
        source: String,
        /// Modality (matches [`super::Observation::modality`]).
        modality: Modality,
        /// Previous payload from the diff baseline.
        from: serde_json::Value,
        /// New payload that triggered the event.
        to: serde_json::Value,
        /// Wall-clock time of the new observation.
        at: SystemTime,
    },

    /// A non-state, one-shot occurrence (file event, key press, audio
    /// transcript chunk, …). No diff/baseline semantics.
    Discrete {
        /// Source name.
        source: String,
        /// Event kind tag (free-form, source-specific).
        kind: String,
        /// Payload.
        data: serde_json::Value,
        /// Wall-clock time.
        at: SystemTime,
    },

    /// A priority signal that bypasses [`super::AttentionGate`] and
    /// [`super::SalienceFilter`] (subject to ack/dedup).  Used for
    /// hardware faults, security-relevant changes, etc.
    Anomaly {
        /// Source name.
        source: String,
        /// Anomaly category.
        reason: AnomalyKind,
        /// Severity in `[0, 255]`. ≥ 128 = blocking, < 128 = informational.
        severity: u8,
        /// Wall-clock time.
        at: SystemTime,
    },

    /// Output of cross-modal fusion — multiple observations bound into a
    /// single semantic entity (e.g. `Window(chrome, focused)`).
    Entity {
        /// The fused entity.
        entity: FusedEntity,
        /// Wall-clock time of the binding event.
        at: SystemTime,
    },
}

impl Event {
    /// Wall-clock timestamp of the event, regardless of variant.
    pub fn at(&self) -> SystemTime {
        match self {
            Event::Change { at, .. }
            | Event::Discrete { at, .. }
            | Event::Anomaly { at, .. }
            | Event::Entity { at, .. } => *at,
        }
    }

    /// Source name when the event has one (`Entity` events do not — they
    /// span sources by design).
    pub fn source(&self) -> Option<&str> {
        match self {
            Event::Change { source, .. }
            | Event::Discrete { source, .. }
            | Event::Anomaly { source, .. } => Some(source.as_str()),
            Event::Entity { .. } => None,
        }
    }

    /// Modality when the event has one (`Discrete`/`Anomaly`/`Entity` may
    /// span modalities or have none).
    pub fn modality(&self) -> Option<Modality> {
        match self {
            Event::Change { modality, .. } => Some(*modality),
            _ => None,
        }
    }

    /// Anomaly events bypass attention/salience gating.
    pub fn is_anomaly(&self) -> bool {
        matches!(self, Event::Anomaly { .. })
    }
}

/// Categories of anomaly events that ride the bypass channel.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AnomalyKind {
    /// A perception source is unhealthy or quarantined.
    SourceFault,
    /// Resource pressure (CPU, memory, disk, network) crossed an alert
    /// threshold.
    ResourcePressure,
    /// File system / configuration change with security implications
    /// (e.g. mass deletion, sensitive file write).
    SecurityEvent,
    /// Hardware presence/absence (device hot-plugged or removed).
    DeviceLifecycle,
    /// Catch-all for anomalies that don't fit other categories.
    Other,
}

/// A sliding-window summary for a `(source, modality)` pair.
///
/// Aggregates are produced by the [`super::TemporalProcessor`] and read
/// via [`super::Snapshot::aggregates`]. The exact contents of `stats`
/// are processor-defined — typical fields: `mean`, `min`, `max`,
/// `last`, `count`, `p95`.
#[derive(Debug, Clone, Serialize)]
pub struct Aggregate {
    /// Source name.
    pub source: String,
    /// Modality.
    pub modality: Modality,
    /// Window duration this aggregate covers (most-recent first).
    #[serde(serialize_with = "serialize_duration_ms")]
    pub window: Duration,
    /// Statistics — open schema. `null` if the modality is non-numeric.
    pub stats: serde_json::Value,
    /// Wall-clock time when this aggregate was computed.
    pub at: SystemTime,
}

fn serialize_duration_ms<S>(d: &Duration, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    s.serialize_u64(d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> SystemTime {
        SystemTime::now()
    }

    #[test]
    fn test_event_at_uniform() {
        let t = now();
        let e = Event::Change {
            source: "s".into(),
            modality: Modality::System,
            from: serde_json::json!(1),
            to: serde_json::json!(2),
            at: t,
        };
        assert_eq!(e.at(), t);
    }

    #[test]
    fn test_event_source_and_modality() {
        let e = Event::Change {
            source: "cpu".into(),
            modality: Modality::System,
            from: serde_json::json!(0),
            to: serde_json::json!(1),
            at: now(),
        };
        assert_eq!(e.source(), Some("cpu"));
        assert_eq!(e.modality(), Some(Modality::System));

        let e = Event::Anomaly {
            source: "mic".into(),
            reason: AnomalyKind::SourceFault,
            severity: 200,
            at: now(),
        };
        assert!(e.is_anomaly());
        assert_eq!(e.source(), Some("mic"));
        assert_eq!(e.modality(), None);
    }

    #[test]
    fn test_event_serializes_with_kind_tag() {
        let e = Event::Discrete {
            source: "fs".into(),
            kind: "modify".into(),
            data: serde_json::json!({"path": "/x"}),
            at: SystemTime::UNIX_EPOCH,
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "discrete");
        assert_eq!(v["source"], "fs");
    }

    #[test]
    fn test_aggregate_window_ms_serializes() {
        let a = Aggregate {
            source: "cpu".into(),
            modality: Modality::System,
            window: Duration::from_millis(2500),
            stats: serde_json::json!({"mean": 12.3}),
            at: SystemTime::UNIX_EPOCH,
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["window"], 2500);
    }
}
