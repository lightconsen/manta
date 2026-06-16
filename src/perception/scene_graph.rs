//! Scene graph — aggregated world state.
//!
//! The [`SceneGraph`] ingests [`Observation`]s and maintains a set of
//! [`Entity`]s keyed by a stable identifier.  Each entity tracks its first
//! and last seen timestamps, modality, confidence, spatial context, and
//! arbitrary properties.

use std::collections::HashMap;
use std::time::Instant;

use crate::perception::{Modality, Observation, SpatialContext};

/// Stable entity identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub struct EntityId(String);

impl EntityId {
    /// Create a new entity ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A relationship between two entities.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Relationship {
    /// Relationship kind, e.g. `"contains"`, `"near"`, `"part_of"`.
    pub kind: String,
    /// Target entity ID.
    pub target: EntityId,
}

/// A tracked entity in the scene graph.
///
/// Entities are created from observations and updated on subsequent observations.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Entity {
    /// Stable identifier.
    pub id: EntityId,
    /// Human-readable label, e.g. `"CPU"`, `"Temperature sensor #1"`.
    pub label: String,
    /// Sensor modality.
    pub modality: Modality,
    /// First observed timestamp.
    #[serde(skip)]
    pub first_seen: Instant,
    /// Most recent observation timestamp.
    #[serde(skip)]
    pub last_seen: Instant,
    /// Current confidence in `[0.0, 1.0]`.
    pub confidence: f32,
    /// Arbitrary properties extracted from observation data.
    pub properties: HashMap<String, serde_json::Value>,
    /// Optional spatial context.
    pub spatial: Option<SpatialContext>,
    /// Relationships to other entities.
    pub relationships: Vec<Relationship>,
}

/// Aggregated world state built from ingested observations.
#[derive(Clone)]
pub struct SceneGraph {
    entities: HashMap<EntityId, Entity>,
}

impl SceneGraph {
    /// Create an empty scene graph.
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
        }
    }

    /// Ingest a single observation, updating or creating entities.
    ///
    /// The observation source name is used as the entity label.  If an entity
    /// with that label already exists, its `last_seen`, `confidence`, and
    /// `properties` are updated.
    pub fn ingest(&mut self, obs: Observation) {
        let entity_id = EntityId::new(obs.source.clone());

        if let Some(entity) = self.entities.get_mut(&entity_id) {
            // Update existing entity
            entity.last_seen = obs.timestamp;
            entity.confidence = obs.confidence;
            entity.properties.insert("data".to_string(), obs.data.clone());
            if let Some(spatial) = obs.spatial {
                entity.spatial = Some(spatial);
            }
        } else {
            // Create new entity
            self.entities.insert(
                entity_id,
                Entity {
                    id: EntityId::new(obs.source),
                    label: format!("{:?}", obs.modality),
                    modality: obs.modality,
                    first_seen: obs.timestamp,
                    last_seen: obs.timestamp,
                    confidence: obs.confidence,
                    properties: {
                        let mut m = HashMap::new();
                        m.insert("data".to_string(), obs.data.clone());
                        m
                    },
                    spatial: obs.spatial,
                    relationships: Vec::new(),
                },
            );
        }
    }

    /// Prune entities not seen since `cutoff`.
    pub fn prune(&mut self, cutoff: Instant) {
        self.entities.retain(|_, e| e.last_seen >= cutoff);
    }

    /// Return all current entities.
    pub fn entities(&self) -> Vec<&Entity> {
        self.entities.values().collect()
    }

    /// Look up a single entity by ID.
    pub fn get(&self, id: &EntityId) -> Option<&Entity> {
        self.entities.get(id)
    }
}

impl Default for SceneGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::{ObservationId, SpatialContext};

    fn make_obs(source: &str, ts: Instant) -> Observation {
        Observation {
            id: ObservationId::new(),
            source: source.to_string(),
            modality: Modality::System,
            timestamp: ts,
            confidence: 1.0,
            spatial: None,
            data: serde_json::json!({"value": 42}),
        }
    }

    #[test]
    fn test_ingest_creates_entity() {
        let mut sg = SceneGraph::new();
        let ts = Instant::now();
        sg.ingest(make_obs("test_sensor", ts));
        assert_eq!(sg.entities().len(), 1);
    }

    #[test]
    fn test_ingest_updates_last_seen() {
        let mut sg = SceneGraph::new();
        let t1 = Instant::now();
        sg.ingest(make_obs("sensor", t1));

        std::thread::sleep(std::time::Duration::from_millis(5));
        let t2 = Instant::now();
        sg.ingest(make_obs("sensor", t2));

        let entity = sg.get(&EntityId::new("sensor")).unwrap();
        assert!(entity.last_seen >= t1);
    }

    #[test]
    fn test_prune_removes_stale() {
        let mut sg = SceneGraph::new();
        sg.ingest(make_obs("old", Instant::now()));
        sg.ingest(make_obs("new", Instant::now()));

        // Prune everything older than "right now"
        let cutoff = Instant::now() + std::time::Duration::from_secs(1);
        sg.prune(cutoff);

        assert!(sg.entities().is_empty());
    }

    #[test]
    fn test_multiple_modalities() {
        let mut sg = SceneGraph::new();
        let ts = Instant::now();

        let mut obs = make_obs("camera", ts);
        obs.modality = Modality::Rgb;
        sg.ingest(obs);

        let mut obs = make_obs("mic", ts);
        obs.modality = Modality::Audio;
        sg.ingest(obs);

        assert_eq!(sg.entities().len(), 2);
    }
}
