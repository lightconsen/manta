//! Perception query types.
//!
//! [`PerceptionQuery`] defines filters for querying the current world state,
//! and [`QueryResult`] carries the matching observations and entities.

use std::time::{Duration, Instant};

use crate::perception::scene_graph::Entity;
use crate::perception::{Modality, Observation};

/// Filter for querying the perception registry and scene graph.
///
/// All fields are `Option`al — unset filters are ignored.
#[derive(Debug, Clone, Default)]
pub struct PerceptionQuery {
    /// Only include entities / observations matching these modalities.
    pub modalities: Option<Vec<Modality>>,
    /// Only include entities / observations matching these source names.
    pub sources: Option<Vec<String>>,
    /// Only consider observations within this look-back window.
    pub time_range: Option<Duration>,
    /// Minimum confidence threshold `[0.0, 1.0]`.
    pub min_confidence: Option<f32>,
    /// Substring match on entity label.
    pub label_contains: Option<String>,
    /// Maximum number of results.
    pub limit: Option<usize>,
}

impl PerceptionQuery {
    /// Returns `true` if the observation matches all set filters.
    pub fn matches_observation(&self, obs: &Observation) -> bool {
        let now = Instant::now();

        if let Some(ref mods) = self.modalities {
            if !mods.contains(&obs.modality) {
                return false;
            }
        }
        if let Some(ref sources) = self.sources {
            if !sources.contains(&obs.source) {
                return false;
            }
        }
        if let Some(range) = self.time_range {
            if now - obs.timestamp > range {
                return false;
            }
        }
        if let Some(min_conf) = self.min_confidence {
            if obs.confidence < min_conf {
                return false;
            }
        }
        true
    }

    /// Returns `true` if the entity matches all set filters.
    pub fn matches_entity(&self, entity: &Entity) -> bool {
        if let Some(ref mods) = self.modalities {
            if !mods.contains(&entity.modality) {
                return false;
            }
        }
        if let Some(ref sources) = self.sources {
            let entity_source = entity.id.to_string();
            if !sources.contains(&entity_source) {
                return false;
            }
        }
        if let Some(ref label) = self.label_contains {
            if !entity.label.contains(label.as_str()) {
                return false;
            }
        }
        if let Some(min_conf) = self.min_confidence {
            if entity.confidence < min_conf {
                return false;
            }
        }
        true
    }
}

/// Result of a perception query.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Matching observations.
    pub observations: Vec<Observation>,
    /// Matching entities from the scene graph.
    pub entities: Vec<Entity>,
    /// The original query.
    pub query: PerceptionQuery,
    /// Query execution timestamp.
    pub timestamp: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::{ObservationId, SpatialContext};

    fn make_obs(source: &str, modality: Modality, ts: Instant) -> Observation {
        Observation {
            id: ObservationId::new(),
            source: source.to_string(),
            modality,
            timestamp: ts,
            confidence: 1.0,
            spatial: None,
            data: serde_json::json!({}),
        }
    }

    #[test]
    fn test_modality_filter() {
        let obs = make_obs("cam", Modality::Rgb, Instant::now());
        let mut q = PerceptionQuery::default();
        q.modalities = Some(vec![Modality::Rgb]);
        assert!(q.matches_observation(&obs));

        q.modalities = Some(vec![Modality::Audio]);
        assert!(!q.matches_observation(&obs));
    }

    #[test]
    fn test_source_filter() {
        let obs = make_obs("camera", Modality::Rgb, Instant::now());
        let mut q = PerceptionQuery::default();
        q.sources = Some(vec!["camera".to_string()]);
        assert!(q.matches_observation(&obs));

        q.sources = Some(vec!["microphone".to_string()]);
        assert!(!q.matches_observation(&obs));
    }

    #[test]
    fn test_time_range_filter() {
        let old = Instant::now() - Duration::from_secs(60);
        let obs = make_obs("cam", Modality::Rgb, old);
        let mut q = PerceptionQuery::default();
        q.time_range = Some(Duration::from_secs(10));
        assert!(!q.matches_observation(&obs));
    }

    #[test]
    fn test_confidence_filter() {
        let mut obs = make_obs("cam", Modality::Rgb, Instant::now());
        obs.confidence = 0.5;
        let mut q = PerceptionQuery::default();
        q.min_confidence = Some(0.8);
        assert!(!q.matches_observation(&obs));

        q.min_confidence = Some(0.3);
        assert!(q.matches_observation(&obs));
    }

    #[test]
    fn test_multiple_filters() {
        let mut obs = make_obs("cam", Modality::Rgb, Instant::now());
        obs.confidence = 0.9;
        let mut q = PerceptionQuery::default();
        q.modalities = Some(vec![Modality::Rgb]);
        q.min_confidence = Some(0.8);
        q.sources = Some(vec!["cam".to_string()]);
        assert!(q.matches_observation(&obs));

        q.sources = Some(vec!["other".to_string()]);
        assert!(!q.matches_observation(&obs));
    }
}
