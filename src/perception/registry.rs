//! Perception registry — manages sources and query routing.
//!
//! The [`PerceptionRegistry`] is the central entry point for the perception
//! fusion layer.  It holds registered [`PerceptionSource`]s, ingests
//! observations into the [`TemporalAggregator`], and serves
//! [`PerceptionQuery`] results.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::broadcast;
use tokio::sync::RwLock;

use crate::perception::{
    AggregationStrategy, Observation, PerceptionQuery, PerceptionSource, QueryResult, SourceStatus,
    TemporalAggregator,
};

/// Central registry for perception sources and the world state.
pub struct PerceptionRegistry {
    sources: RwLock<HashMap<String, Arc<dyn PerceptionSource>>>,
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

        // Ingest into aggregator
        let mut aggregator = self.aggregator.write().await;
        for obs in all_obs {
            aggregator.push(obs);
        }
    }

    /// Query the current aggregated entities and observation history.
    pub async fn query(&self, q: &PerceptionQuery) -> QueryResult {
        let aggregator = self.aggregator.read().await;

        let entities: Vec<_> = aggregator
            .aggregate()
            .into_iter()
            .filter(|e| q.matches_entity(e))
            .collect();

        let observations: Vec<Observation> = aggregator
            .observations()
            .into_iter()
            .filter(|obs| q.matches_observation(obs))
            .cloned()
            .collect();

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

    /// Return the status of each registered source as a map of source name → status.
    pub async fn list_source_statuses(&self) -> std::collections::HashMap<String, SourceStatus> {
        let sources = self.sources.read().await;
        sources
            .iter()
            .map(|(name, source)| (name.clone(), source.status()))
            .collect()
    }

    /// Return all observations currently in the aggregation window.
    pub async fn all_observations(&self) -> Vec<Observation> {
        self.aggregator.read().await.observations().into_iter().cloned().collect()
    }

    /// Remove a single source by exact name.
    pub async fn deregister_source(&self, name: &str) {
        self.sources.write().await.remove(name);
    }

    /// Remove all sources whose names start with `prefix`.
    ///
    /// Used during hotplug removal or config reload to clean up all sources
    /// belonging to a device or subsystem.
    pub async fn deregister_prefix(&self, prefix: &str) {
        self.sources.write().await.retain(|k, _| !k.starts_with(prefix));
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
    async fn test_deregister_source_removes_single() {
        let reg = PerceptionRegistry::new(AggregationStrategy::Latest, 10);
        reg.register_source(Arc::new(MockPerceptionSource::new("alpha"))).await;
        reg.register_source(Arc::new(MockPerceptionSource::new("beta"))).await;
        assert_eq!(reg.list_sources().await.len(), 2);

        reg.deregister_source("alpha").await;
        let names = reg.list_sources().await;
        assert_eq!(names.len(), 1);
        assert!(names.contains(&"beta".to_string()));
    }

    #[tokio::test]
    async fn test_deregister_prefix_removes_matching() {
        let reg = PerceptionRegistry::new(AggregationStrategy::Latest, 10);
        reg.register_source(Arc::new(MockPerceptionSource::new("device:temp:1"))).await;
        reg.register_source(Arc::new(MockPerceptionSource::new("device:pressure:2"))).await;
        reg.register_source(Arc::new(MockPerceptionSource::new("screenshot"))).await;

        reg.deregister_prefix("device:").await;
        let names = reg.list_sources().await;
        assert_eq!(names.len(), 1);
        assert!(names.contains(&"screenshot".to_string()));
    }

    #[tokio::test]
    async fn test_deregister_nonexistent_is_noop() {
        let reg = PerceptionRegistry::new(AggregationStrategy::Latest, 10);
        reg.register_source(Arc::new(MockPerceptionSource::new("only_source"))).await;
        reg.deregister_source("does_not_exist").await;
        assert_eq!(reg.list_sources().await.len(), 1);
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
