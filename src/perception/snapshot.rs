//! [`Snapshot`] — the synchronous, agent-facing view of the world.
//!
//! A `Snapshot` is what an agent calls
//! [`super::AgentPerceptionAdapter::now`] to get. It bundles three
//! things derived from the upstream pipeline:
//!
//! 1. **Entities** — current cross-modal fusion state.
//! 2. **Aggregates** — sliding-window stats per `(source, modality)`.
//! 3. **Recent events** — events still queued in the adapter that the
//!    agent hasn't consumed via [`super::AgentPerceptionAdapter::next_event`].
//!
//! Snapshots are **read-only and cheap to produce** — taking a snapshot
//! does not consume queued events; it only mirrors them. The agent is
//! free to call `now()` repeatedly, e.g. once per LLM turn.

use std::collections::HashMap;
use std::time::SystemTime;

use serde::Serialize;

use crate::perception::{Aggregate, Event, FusedEntity, Modality};

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
            && self.summary.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true)
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
            // Stable ordering: sort by (source, modality) string repr.
            let mut keys: Vec<&(String, Modality)> = self.aggregates.keys().collect();
            keys.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| format!("{:?}", a.1).cmp(&format!("{:?}", b.1))));
            for key in keys {
                if let Some(agg) = self.aggregates.get(key) {
                    out.push_str(&format!(
                        "- {} ({:?}): {}\n",
                        key.0,
                        key.1,
                        agg.stats,
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
    use super::*;
    use std::time::Duration;

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
}
