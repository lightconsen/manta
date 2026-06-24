//! [`Snapshot`] — the synchronous, agent-facing view of the world.
//!
//! A `Snapshot` is what an agent calls
//! [`super::AgentPerceptionAdapter::now`] to get. It bundles three
//! things derived from the upstream pipeline:
//!
//! 1. **Entities** — current cross-modal fusion state.
//! 2. **Aggregates** — sliding-window stats per `(source, modality)`.
//! 3. **Recent events** — events still queued in the adapter that the agent
//!    hasn't consumed via [`super::AgentPerceptionAdapter::next_event`].
//!
//! Snapshots are **read-only and cheap to produce** — taking a snapshot
//! does not consume queued events; it only mirrors them. The agent is
//! free to call `now()` repeatedly, e.g. once per LLM turn.

use std::collections::HashMap;
use std::time::SystemTime;

use serde::Serialize;

use crate::perception::{Aggregate, Event, FusedEntity, Modality};

/// Maximum number of sensor aggregates rendered per modality in
/// [`Snapshot::format_for_prompt`]. Caps token usage when a single
/// modality has many sources; excess entries are summarized as a
/// `… +N more` line.
const MAX_SENSORS_PER_MODALITY: usize = 5;

/// Snapshot of the world according to perception, at a single instant.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// Wall-clock time the snapshot was produced.
    pub at: SystemTime,
    /// Currently bound cross-modal entities.
    pub entities: Vec<FusedEntity>,
    /// Sliding-window aggregates keyed by `(source, modality)`.
    ///
    /// Serialized as a list to avoid the JSON object-key restriction
    /// (`(String, Modality)` cannot be a JSON map key).
    #[serde(serialize_with = "serialize_aggregate_map")]
    pub aggregates: HashMap<(String, Modality), Aggregate>,
    /// Most recent events still queued in the per-agent adapter.
    /// Bounded by adapter configuration (typical: last 64 events).
    pub recent_events: Vec<Event>,
    /// Optional LLM-generated narrative summary of the recent
    /// environment, refreshed on a configurable cadence by the adapter.
    /// `None` when no summarizer is configured or the first refresh
    /// hasn't completed yet.
    pub summary: Option<String>,
}

impl Snapshot {
    /// Create an empty snapshot stamped `now`.
    pub fn empty() -> Self {
        Self {
            at: SystemTime::now(),
            entities: Vec::new(),
            aggregates: HashMap::new(),
            recent_events: Vec::new(),
            summary: None,
        }
    }

    /// Number of entities + queued events. Useful for logging /
    /// telemetry (high counts = adapter under-consumed).
    pub fn item_count(&self) -> usize {
        self.entities.len() + self.recent_events.len()
    }

    /// Format the snapshot as a compact, prompt-ready Markdown block
    /// suitable for splicing into an agent's system prompt.
    ///
    /// Returns `None` if there is nothing worth showing (no entities,
    /// no aggregates, no recent events) — callers can use this to
    /// avoid emitting an empty `## Perception` section.
    ///
    /// `max_recent` caps how many recent events are listed (oldest
    /// first; typical: 8). The output is intentionally short so it
    /// doesn't bloat the prompt.
    pub fn format_for_prompt(&self, max_recent: usize) -> Option<String> {
        if self.entities.is_empty()
            && self.aggregates.is_empty()
            && self.recent_events.is_empty()
            && self
                .summary
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
        {
            return None;
        }
        let mut out = String::from("## Perception\n");

        if let Some(s) = self.summary.as_ref() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                out.push_str("\n### Summary\n");
                out.push_str(trimmed);
                out.push('\n');
            }
        }

        if !self.aggregates.is_empty() {
            out.push_str("\n### Sensors (current)\n");
            // Group by modality, then cap each modality to the top-N
            // entries (sorted by source name) so a chatty modality can't
            // blow up the prompt token budget. Stable ordering throughout.
            let mut by_modality: HashMap<Modality, Vec<&(String, Modality)>> = HashMap::new();
            for key in self.aggregates.keys() {
                by_modality.entry(key.1).or_default().push(key);
            }
            let mut modalities: Vec<Modality> = by_modality.keys().copied().collect();
            modalities.sort_by_key(|m| format!("{m:?}"));

            for modality in modalities {
                #[allow(clippy::expect_used)] // modality came from by_modality.keys() above
                let keys = by_modality.get_mut(&modality).expect("modality present");
                keys.sort_by(|a, b| a.0.cmp(&b.0));
                let total = keys.len();
                for key in keys.iter().take(MAX_SENSORS_PER_MODALITY) {
                    if let Some(agg) = self.aggregates.get(*key) {
                        out.push_str(&format!("- {} ({:?}): {}\n", key.0, key.1, agg.stats));
                    }
                }
                if total > MAX_SENSORS_PER_MODALITY {
                    out.push_str(&format!(
                        "- … +{} more {:?} sensor(s)\n",
                        total - MAX_SENSORS_PER_MODALITY,
                        modality,
                    ));
                }
            }
        }

        if !self.entities.is_empty() {
            out.push_str("\n### Entities\n");
            for e in &self.entities {
                out.push_str(&format!(
                    "- {} ({:?}, conf={:.2})\n",
                    e.label, e.modalities, e.confidence,
                ));
            }
        }

        if !self.recent_events.is_empty() {
            out.push_str("\n### Recent events\n");
            let n = self.recent_events.len();
            let start = n.saturating_sub(max_recent);
            for ev in &self.recent_events[start..] {
                out.push_str(&format!("- {}\n", ev.short_label()));
            }
        }

        Some(out)
    }
}

fn serialize_aggregate_map<S>(
    map: &HashMap<(String, Modality), Aggregate>,
    s: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeSeq;
    let mut seq = s.serialize_seq(Some(map.len()))?;
    for ((source, modality), agg) in map {
        seq.serialize_element(&AggregateEntry {
            source,
            modality: *modality,
            aggregate: agg,
        })?;
    }
    seq.end()
}

#[derive(Serialize)]
struct AggregateEntry<'a> {
    source: &'a str,
    modality: Modality,
    #[serde(flatten)]
    aggregate: &'a Aggregate,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn test_empty_snapshot() {
        let s = Snapshot::empty();
        assert_eq!(s.item_count(), 0);
        assert!(s.entities.is_empty());
        assert!(s.recent_events.is_empty());
    }

    #[test]
    fn test_snapshot_serializes() {
        let mut s = Snapshot::empty();
        s.aggregates.insert(
            ("cpu".to_string(), Modality::System),
            Aggregate {
                source: "cpu".into(),
                modality: Modality::System,
                window: Duration::from_secs(2),
                stats: serde_json::json!({"mean": 12.3}),
                at: SystemTime::UNIX_EPOCH,
            },
        );
        let v = serde_json::to_value(&s).unwrap();
        assert!(v["aggregates"].is_array());
        assert_eq!(v["aggregates"][0]["source"], "cpu");
    }

    #[test]
    fn test_sensors_truncated_per_modality() {
        let mut s = Snapshot::empty();
        // 8 System sensors — exceeds MAX_SENSORS_PER_MODALITY (5).
        for i in 0..8 {
            s.aggregates.insert(
                (format!("sensor_{i:02}"), Modality::System),
                Aggregate {
                    source: format!("sensor_{i:02}"),
                    modality: Modality::System,
                    window: Duration::from_secs(2),
                    stats: serde_json::json!({"v": i}),
                    at: SystemTime::UNIX_EPOCH,
                },
            );
        }
        let out = s.format_for_prompt(8).expect("non-empty snapshot");
        // First 5 (sorted) are present; the 6th onward are not.
        assert!(out.contains("sensor_00"));
        assert!(out.contains("sensor_04"));
        assert!(!out.contains("sensor_05"));
        assert!(out.contains("+3 more"));
    }
}
