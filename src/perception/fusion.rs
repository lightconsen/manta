//! Cross-modal sensor fusion engine.
//!
//! The [`FusionEngine`] takes a set of [`Observation`]s from multiple
//! perception sources and produces [`FusedEntity`]s by correlating them
//! across temporal and modality dimensions.
//!
//! # Fusion pipeline
//!
//! 1. **Filter** — Remove observations below confidence threshold.
//! 2. **Temporal clustering** — Cluster observations within a configurable time
//!    window.
//! 3. **Conflict resolution** — Within each cluster, per modality: pick the
//!    observation with the highest confidence (tiebreak by recency).
//! 4. **Entity building** — Merge properties and metadata into a
//!    [`FusedEntity`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::perception::{Modality, Observation};

/// A fused entity that references sub-observations from multiple modalities.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FusedEntity {
    /// Unique identifier for this fused entity.
    pub id: String,
    /// Human-readable label derived from the constituent observations.
    pub label: String,
    /// When the fused entity was first created.
    #[serde(skip)]
    pub created_at: Instant,
    /// When the fused entity was last updated.
    #[serde(skip)]
    pub updated_at: Instant,
    /// Overall confidence (aggregated from sub-observations).
    pub confidence: f32,
    /// Modalities that contributed to this entity.
    pub modalities: Vec<Modality>,
    /// IDs of the sub-observations that contributed.
    pub observation_ids: Vec<String>,
    /// Combined properties from all sub-observations, keyed by modality name.
    pub properties: HashMap<String, serde_json::Value>,
    /// The correlation key used to group these observations (e.g. temporal
    /// location label or temporal cluster ID).
    pub correlation_key: String,
}

/// Configuration for the fusion engine.
#[derive(Debug, Clone)]
pub struct FusionConfig {
    /// Maximum time window in milliseconds for temporal correlation.
    /// Observations within this window are candidates for fusion.
    pub temporal_window_ms: u64,
    /// Minimum confidence to include an observation in fusion.
    /// Observations below this threshold are excluded.
    pub min_confidence: f32,
}

impl Default for FusionConfig {
    fn default() -> Self {
        Self {
            temporal_window_ms: 500,
            min_confidence: 0.3,
        }
    }
}

/// Cross-modal fusion engine.
///
/// `fuse()` reads the current config under a short read-lock, then operates
/// on a snapshot — concurrent `update_config` calls do not interrupt an
/// in-flight fuse.
#[derive(Debug, Clone)]
pub struct FusionEngine {
    config: Arc<RwLock<FusionConfig>>,
}

impl FusionEngine {
    /// Create a new fusion engine with the given configuration.
    pub fn new(config: FusionConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
        }
    }

    /// Snapshot the current config (cheap clone).
    pub async fn config(&self) -> FusionConfig {
        self.config.read().await.clone()
    }

    /// Atomically update the config under the writer lock.
    pub async fn update_config<F>(&self, f: F)
    where
        F: FnOnce(&mut FusionConfig),
    {
        let mut g = self.config.write().await;
        f(&mut g);
    }

    /// Replace the config in one shot.
    pub async fn set_config(&self, new_cfg: FusionConfig) {
        *self.config.write().await = new_cfg;
    }

    /// Synchronous variant used inside `fuse()` after taking a snapshot.
    fn fuse_with_config(
        &self,
        cfg: &FusionConfig,
        observations: &[Observation],
    ) -> Vec<FusedEntity> {
        if observations.is_empty() {
            return vec![];
        }

        let filtered: Vec<&Observation> = observations
            .iter()
            .filter(|obs| obs.confidence >= cfg.min_confidence)
            .collect();

        if filtered.is_empty() {
            return vec![];
        }

        let clusters = Self::cluster_by_time(cfg.temporal_window_ms, &filtered);
        let mut fused = Vec::new();
        for cluster in clusters {
            if let Some(entity) = self.build_fused_entity(cluster) {
                fused.push(entity);
            }
        }
        fused
    }

    /// Fuse a set of observations into cross-modal entities.
    ///
    /// Returns a list of [`FusedEntity`]s, one per temporal cluster that
    /// contains at least one observation meeting the confidence threshold.
    pub async fn fuse(&self, observations: &[Observation]) -> Vec<FusedEntity> {
        let cfg = self.config.read().await.clone();
        self.fuse_with_config(&cfg, observations)
    }

    /// Synchronous fuse — uses the last-known config without awaiting.
    /// Available because [`FusionEngine::config`] is held in an
    /// [`Arc<RwLock>`]. Useful from synchronous contexts (e.g. existing
    /// test code).
    pub fn fuse_blocking(&self, observations: &[Observation]) -> Vec<FusedEntity> {
        let cfg = self.config.blocking_read().clone();
        self.fuse_with_config(&cfg, observations)
    }

    /// Cluster observations within a time window (temporal correlation).
    ///
    /// Uses a greedy single-pass algorithm:
    /// - Sort observations by timestamp.
    /// - Start a new cluster at the first observation.
    /// - Add subsequent observations to the current cluster if they fall within
    ///   `temporal_window_ms` of the cluster start.
    /// - Otherwise, start a new cluster.
    fn cluster_by_time<'a>(
        temporal_window_ms: u64,
        observations: &[&'a Observation],
    ) -> Vec<Vec<&'a Observation>> {
        if observations.is_empty() {
            return vec![];
        }

        let mut sorted: Vec<&'a Observation> = observations.to_vec();
        sorted.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        let window = Duration::from_millis(temporal_window_ms);
        let mut clusters: Vec<Vec<&'a Observation>> = Vec::new();
        let mut current_cluster: Vec<&'a Observation> = Vec::new();
        let mut window_start: Instant = sorted[0].timestamp;

        for obs in sorted {
            if obs.timestamp.duration_since(window_start) <= window {
                current_cluster.push(obs);
            } else {
                // Finalize the current cluster and start a new one
                if !current_cluster.is_empty() {
                    clusters.push(std::mem::take(&mut current_cluster));
                }
                current_cluster.push(obs);
                window_start = obs.timestamp;
            }
        }

        // Don't forget the last cluster
        if !current_cluster.is_empty() {
            clusters.push(current_cluster);
        }

        clusters
    }

    /// For a single cluster, resolve conflicts and build a [`FusedEntity`].
    ///
    /// Conflict resolution: per modality, pick the observation with the
    /// highest confidence; tiebreak by most recent timestamp.
    fn resolve_conflicts<'a>(
        &self,
        cluster: Vec<&'a Observation>,
    ) -> HashMap<Modality, &'a Observation> {
        let mut winners: HashMap<Modality, &'a Observation> = HashMap::new();

        for obs in cluster {
            let modality = obs.modality;
            match winners.get(&modality) {
                Some(current) => {
                    // Higher confidence wins; tiebreak by recency
                    if obs.confidence > current.confidence
                        || (obs.confidence == current.confidence
                            && obs.timestamp > current.timestamp)
                    {
                        winners.insert(modality, obs);
                    }
                }
                None => {
                    winners.insert(modality, obs);
                }
            }
        }

        winners
    }

    /// Build a [`FusedEntity`] from a cluster of observations after
    /// conflict resolution.
    fn build_fused_entity(&self, cluster: Vec<&Observation>) -> Option<FusedEntity> {
        if cluster.is_empty() {
            return None;
        }

        let winners = self.resolve_conflicts(cluster);

        // Derive aggregate properties
        let now = Instant::now();
        let mut modalities: Vec<Modality> = winners.keys().copied().collect();
        modalities.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));

        let observation_ids: Vec<String> = winners.values().map(|obs| obs.id.to_string()).collect();

        // Aggregate confidence: weighted average by count (simple approach)
        let confidence =
            winners.values().map(|obs| obs.confidence).sum::<f32>() / winners.len().max(1) as f32;

        // Merge properties keyed by modality name
        let mut properties: HashMap<String, serde_json::Value> = HashMap::new();
        for obs in winners.values() {
            let key = format!("{:?}", obs.modality);
            properties.insert(key, obs.data.clone());
        }

        // Label from modality combination
        let mod_strs: Vec<String> = modalities.iter().map(|m| format!("{m:?}")).collect();
        let label = format!("Fused({})", mod_strs.join("+"));

        // Correlation key always "temporal" (no spatial alignment)
        let correlation_key = "temporal".to_string();

        Some(FusedEntity {
            id: observation_ids
                .first()
                .map(|id| id.to_string())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            label,
            created_at: now,
            updated_at: now,
            confidence,
            modalities,
            observation_ids,
            properties,
            correlation_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::ObservationId;

    fn make_obs(source: &str, modality: Modality, ts: Instant, conf: f32) -> Observation {
        Observation {
            id: ObservationId::new(),
            source: source.to_string(),
            modality,
            timestamp: ts,
            created_at: std::time::SystemTime::now(),
            confidence: conf,
            data: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn test_fuse_two_modalities_same_cluster() {
        let engine = FusionEngine::new(FusionConfig::default());
        let now = Instant::now();
        let obs = vec![
            make_obs("camera", Modality::Rgb, now, 0.9),
            make_obs("mic", Modality::Audio, now, 0.8),
        ];
        let fused = engine.fuse(&obs).await;
        assert_eq!(fused.len(), 1, "expected 1 fused entity");
        assert_eq!(fused[0].modalities.len(), 2);
    }

    #[tokio::test]
    async fn test_conflict_resolution_higher_confidence_wins() {
        let engine = FusionEngine::new(FusionConfig::default());
        let now = Instant::now();
        let obs = vec![
            make_obs("cam1", Modality::Rgb, now, 0.6),
            make_obs("cam2", Modality::Rgb, now, 0.9),
        ];
        let fused = engine.fuse(&obs).await;
        assert!(!fused.is_empty());
        assert!(
            (fused[0].confidence - 0.9).abs() < 0.01,
            "confidence should be ~0.9 (the higher of the two)"
        );
    }

    #[tokio::test]
    async fn test_temporal_window_excludes_old_observations() {
        let engine = FusionEngine::new(FusionConfig {
            temporal_window_ms: 100,
            min_confidence: 0.0,
        });
        let now = Instant::now();
        let old = now - Duration::from_secs(1);
        let obs = vec![
            make_obs("cam", Modality::Rgb, now, 0.9),
            make_obs("old_mic", Modality::Audio, old, 0.8),
        ];
        let fused = engine.fuse(&obs).await;
        assert!(!fused.is_empty());
        // Only Rgb modality should be in the recent cluster
        assert!(
            fused.iter().any(|e| e.modalities == vec![Modality::Rgb]),
            "old_mic should be in a separate temporal cluster"
        );
    }

    #[tokio::test]
    async fn test_min_confidence_filter() {
        let engine = FusionEngine::new(FusionConfig {
            temporal_window_ms: 1000,
            min_confidence: 0.7,
        });
        let now = Instant::now();
        let obs = vec![
            make_obs("good_cam", Modality::Rgb, now, 0.9),
            make_obs("noisy_mic", Modality::Audio, now, 0.3),
        ];
        let fused = engine.fuse(&obs).await;
        assert!(!fused.is_empty());
        assert!(
            fused.iter().all(|e| e.modalities == vec![Modality::Rgb]),
            "noisy_mic should be filtered out by min_confidence"
        );
    }

    #[tokio::test]
    async fn test_fuse_empty_observations() {
        let engine = FusionEngine::new(FusionConfig::default());
        let fused = engine.fuse(&[]).await;
        assert!(fused.is_empty());
    }

    #[tokio::test]
    async fn test_all_below_min_confidence_returns_empty() {
        let engine = FusionEngine::new(FusionConfig {
            min_confidence: 0.8,
            ..Default::default()
        });
        let now = Instant::now();
        let obs = vec![
            make_obs("noisy", Modality::Rgb, now, 0.2),
            make_obs("noisy2", Modality::Audio, now, 0.3),
        ];
        let fused = engine.fuse(&obs).await;
        assert!(fused.is_empty());
    }

    #[tokio::test]
    async fn test_fused_entity_contains_observation_ids() {
        let engine = FusionEngine::new(FusionConfig::default());
        let now = Instant::now();
        let obs = vec![
            make_obs("cam", Modality::Rgb, now, 0.9),
            make_obs("mic", Modality::Audio, now, 0.8),
        ];
        let fused = engine.fuse(&obs).await;
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].observation_ids.len(), 2);
    }

    #[tokio::test]
    async fn test_update_config_changes_behavior() {
        let engine = FusionEngine::new(FusionConfig {
            min_confidence: 0.0,
            temporal_window_ms: 500,
        });
        let now = Instant::now();
        let obs = vec![make_obs("noisy", Modality::Rgb, now, 0.3)];
        // Initially passes
        assert_eq!(engine.fuse(&obs).await.len(), 1);
        // Tighten threshold and re-fuse
        engine.update_config(|c| c.min_confidence = 0.9).await;
        assert!(engine.fuse(&obs).await.is_empty());
    }
}
