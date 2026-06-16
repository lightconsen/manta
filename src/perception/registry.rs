//! Perception registry — manages sources, scene graph, and query routing.
//!
//! The [`PerceptionRegistry`] is the central entry point for the perception
//! fusion layer.  It holds registered [`PerceptionSource`]s, ingests
//! observations into the [`SceneGraph`], and serves [`PerceptionQuery`] results.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::broadcast;
use tokio::sync::RwLock;

use crate::perception::{
    AggregationStrategy, Observation, PerceptionQuery, PerceptionSource, QueryResult, SceneGraph,
    TemporalAggregator,
};

/// Central registry for perception sources and the world state.
pub struct PerceptionRegistry {
    sources: RwLock<HashMap<String, Arc<dyn PerceptionSource>>>,
    scene_graph: RwLock<SceneGraph>,
    aggregator: RwLock<TemporalAggregator>,
}

impl PerceptionRegistry {
    /// Create a new empty registry with default aggregation.
    pub fn new(
        aggregation_strategy: AggregationStrategy,
        window_secs: u64,
    ) -> Self {
        Self {
            sources: RwLock::new(HashMap::new()),
            scene_graph: RwLock::new(SceneGraph::new()),
            aggregator: RwLock::new(TemporalAggregator::new(
                aggregation_strategy,
                std::time::Duration::from_secs(window_secs),
            )),
        }
    }

    /// Register a perception source.
    pub async fn register_source(&self, source: Arc<dyn PerceptionSource>) {
        let mut sources = self.sources.write().await;
        sources.insert(source.name().to_string(), source);
    }

    /// Poll all registered sources and ingest observations.
    pub async fn poll_all(&self) {
        let sources = self.sources.read().await;
        let mut all_obs = Vec::new();

        for (_, source) in sources.iter() {
            let obs = source.observe().await;
            all_obs.extend(obs);
        }
        drop(sources);

        // Ingest into aggregator and scene graph
        let mut aggregator = self.aggregator.write().await;
        let mut scene_graph = self.scene_graph.write().await;

        for obs in all_obs {
            aggregator.push(obs.clone());
            scene_graph.ingest(obs);
        }
    }

    /// Query the current scene graph and observation history.
    pub async fn query(&self, q: &PerceptionQuery) -> QueryResult {
        let scene_graph = self.scene_graph.read().await;
        let _aggregator = self.aggregator.read().await;

        let entities: Vec<_> = scene_graph
            .entities()
            .into_iter()
            .filter(|e| q.matches_entity(e))
            .cloned()
            .collect();

        // For observations we'd ideally iterate over the aggregator's window,
        // but since it doesn't expose raw observations directly, we return
        // what we have from entity matches.
        let observations: Vec<Observation> = Vec::new();

        let mut result = QueryResult {
            observations,
            entities,
            query: q.clone(),
            timestamp: Instant::now(),
        };

        if let Some(limit) = q.limit {
            result.observations.truncate(limit);
            result.entities.truncate(limit);
        }

        result
    }

    /// Subscribe to observations from a specific source.
    pub async fn subscribe(&self, source_name: &str) -> Option<broadcast::Receiver<Observation>> {
        let sources = self.sources.read().await;
        let source = sources.get(source_name)?;
        source.subscribe()
    }

    /// List all registered source names.
    pub async fn list_sources(&self) -> Vec<String> {
        let sources = self.sources.read().await;
        sources.keys().cloned().collect()
    }

    /// Access the scene graph (for inspection / testing).
    pub async fn scene_graph(&self) -> SceneGraph {
        self.scene_graph.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::mock::MockPerceptionSource;
    use crate::perception::Modality;

    #[tokio::test]
    async fn test_register_and_list() {
        let reg = PerceptionRegistry::new(AggregationStrategy::Latest, 10);
        let src = Arc::new(MockPerceptionSource::new("test_src"));
        reg.register_source(src).await;
        let names = reg.list_sources().await;
        assert!(names.contains(&"test_src".to_string()));
    }

    #[tokio::test]
    async fn test_poll_all_and_query() {
        let reg = PerceptionRegistry::new(AggregationStrategy::Latest, 10);
        let src = Arc::new(
            MockPerceptionSource::new("sensor_a")
                .with_modality(Modality::Device)
                .with_data(serde_json::json!({"value": 100})),
        );
        reg.register_source(src).await;
        reg.poll_all().await;

        let mut q = PerceptionQuery::default();
        q.modalities = Some(vec![Modality::Device]);
        let result = reg.query(&q).await;
        assert!(!result.entities.is_empty(), "expected entities after poll");
    }

    #[tokio::test]
    async fn test_query_by_source() {
        let reg = PerceptionRegistry::new(AggregationStrategy::Latest, 10);
        let src = Arc::new(
            MockPerceptionSource::new("sensor_b")
                .with_modality(Modality::System)
                .with_data(serde_json::json!({"cpu": 45})),
        );
        reg.register_source(src).await;
        reg.poll_all().await;

        let mut q = PerceptionQuery::default();
        q.sources = Some(vec!["sensor_b".to_string()]);
        let result = reg.query(&q).await;
        assert!(!result.entities.is_empty(), "expected entities for sensor_b");

        q.sources = Some(vec!["nonexistent".to_string()]);
        let result = reg.query(&q).await;
        assert!(result.entities.is_empty(), "expected no entities for unknown source");
    }
}
