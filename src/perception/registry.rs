//! Perception registry — manages sources and query routing.
//!
//! The [`PerceptionRegistry`] is the central entry point for the perception
//! fusion layer.  It holds registered [`PerceptionSource`]s, ingests
//! observations into the [`TemporalAggregator`], and serves
//! [`PerceptionQuery`] results.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::broadcast;
use tokio::sync::RwLock;

use crate::perception::health::{HealthConfig, HealthTracker, SourceHealth};
use crate::perception::persistence::{NullObservationStore, ObservationStore};
use crate::perception::{
    AggregationStrategy, Observation, PerceptionQuery, PerceptionSource, QueryResult, SourceStatus,
    TemporalAggregator,
};

/// Central registry for perception sources and the world state.
pub struct PerceptionRegistry {
    sources: RwLock<HashMap<String, Arc<dyn PerceptionSource>>>,
    aggregator: RwLock<TemporalAggregator>,
    health: RwLock<HealthTracker>,
    /// Durable observation store. Defaults to [`NullObservationStore`] (no-op).
    store: Arc<dyn ObservationStore>,
}

impl PerceptionRegistry {
    /// Create a new empty registry with default aggregation and health config.
    pub fn new(
        aggregation_strategy: AggregationStrategy,
        window_secs: u64,
    ) -> Self {
        Self::with_health_config(aggregation_strategy, window_secs, HealthConfig::default())
    }

    /// Create a new empty registry with a custom health config.
    pub fn with_health_config(
        aggregation_strategy: AggregationStrategy,
        window_secs: u64,
        health_config: HealthConfig,
    ) -> Self {
        Self {
            sources: RwLock::new(HashMap::new()),
            aggregator: RwLock::new(TemporalAggregator::new(
                aggregation_strategy,
                std::time::Duration::from_secs(window_secs),
            )),
            health: RwLock::new(HealthTracker::new(health_config)),
            store: Arc::new(NullObservationStore),
        }
    }

    /// Replace the durable observation store. By default a [`NullObservationStore`]
    /// is used; call this once at construction to enable persistence.
    pub fn with_store(mut self, store: Arc<dyn ObservationStore>) -> Self {
        self.store = store;
        self
    }

    /// Access the durable observation store (for queries that span beyond the
    /// in-memory aggregation window).
    pub fn store(&self) -> Arc<dyn ObservationStore> {
        self.store.clone()
    }

    /// Register a perception source.
    pub async fn register_source(&self, source: Arc<dyn PerceptionSource>) {
        let name = source.name().to_string();
        self.sources.write().await.insert(name.clone(), source);
        self.health.write().await.touch(&name);
    }

    /// Poll all registered sources in parallel with per-source timeouts.
    ///
    /// Quarantined sources are skipped. A timeout or error on one source
    /// does not block or affect others. Successes/failures are recorded in
    /// the [`HealthTracker`], which adapts each source's timeout/interval.
    pub async fn poll_all(&self) {
        // Snapshot the source list and per-source timeouts so we don't hold
        // the read lock while polling.
        let sources_snapshot: Vec<(String, Arc<dyn PerceptionSource>)> = {
            let sources = self.sources.read().await;
            sources
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        let (timeouts, quarantined): (HashMap<String, std::time::Duration>, std::collections::HashSet<String>) = {
            let h = self.health.read().await;
            let timeouts = sources_snapshot
                .iter()
                .map(|(k, _)| (k.clone(), h.timeout_for(k)))
                .collect();
            let quarantined = sources_snapshot
                .iter()
                .filter(|(k, _)| h.is_quarantined(k))
                .map(|(k, _)| k.clone())
                .collect();
            (timeouts, quarantined)
        };

        // Launch all observe() calls in parallel.
        let mut futs = FuturesUnordered::new();
        for (name, src) in sources_snapshot {
            if quarantined.contains(&name) {
                continue;
            }
            let timeout = timeouts
                .get(&name)
                .copied()
                .unwrap_or_else(|| std::time::Duration::from_secs(2));
            futs.push(async move {
                let r = tokio::time::timeout(timeout, src.observe()).await;
                (name, r)
            });
        }

        // Drain results, updating health and ingesting into the aggregator.
        let mut all_obs = Vec::new();
        while let Some((name, result)) = futs.next().await {
            match result {
                Ok(obs) => {
                    self.health.write().await.record_success(&name);
                    all_obs.extend(obs);
                }
                Err(_elapsed) => {
                    tracing::warn!("perception source '{}' poll timed out", name);
                    self.health.write().await.record_timeout(&name);
                }
            }
        }

        let mut aggregator = self.aggregator.write().await;
        for obs in &all_obs {
            aggregator.push(obs.clone());
        }
        drop(aggregator);

        // Best-effort durable persistence — log on error but never propagate,
        // as the in-memory window is the source of truth for live queries.
        if !all_obs.is_empty() {
            if let Err(e) = self.store.append_batch(&all_obs).await {
                tracing::warn!("perception store append failed: {}", e);
            }
        }
    }

    /// Probe a quarantined source once (single observe attempt).
    ///
    /// On success the source is moved back to `Healthy` with default
    /// timeouts. On failure it stays quarantined with the same backoff.
    /// Useful for periodic recovery probes from a background task.
    pub async fn probe_source(&self, name: &str) -> bool {
        let source = match self.sources.read().await.get(name).cloned() {
            Some(s) => s,
            None => return false,
        };
        let timeout = self.health.read().await.timeout_for(name);
        let result = tokio::time::timeout(timeout, source.observe()).await;
        match result {
            Ok(obs) => {
                self.health.write().await.record_success(name);
                let mut aggregator = self.aggregator.write().await;
                for o in &obs {
                    aggregator.push(o.clone());
                }
                drop(aggregator);
                if !obs.is_empty() {
                    if let Err(e) = self.store.append_batch(&obs).await {
                        tracing::warn!("perception store append failed (probe): {}", e);
                    }
                }
                true
            }
            Err(_) => {
                self.health.write().await.record_timeout(name);
                false
            }
        }
    }

    /// Snapshot of per-source health metrics.
    pub async fn health_snapshot(&self) -> HashMap<String, SourceHealth> {
        self.health.read().await.snapshot()
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

    /// Return a clone of the registered source list (name → source).
    ///
    /// Used by [`PerceptionStreamHub`](crate::perception::stream::PerceptionStreamHub)
    /// to wire forward tasks for each streaming source.
    pub async fn sources_snapshot(&self) -> Vec<(String, Arc<dyn PerceptionSource>)> {
        self.sources
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Remove a single source by exact name.
    pub async fn deregister_source(&self, name: &str) {
        self.sources.write().await.remove(name);
        self.health.write().await.forget(name);
    }

    /// Remove all sources whose names start with `prefix`.
    ///
    /// Used during hotplug removal or config reload to clean up all sources
    /// belonging to a device or subsystem.
    pub async fn deregister_prefix(&self, prefix: &str) {
        let removed: Vec<String> = {
            let mut sources = self.sources.write().await;
            let names: Vec<String> = sources
                .keys()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect();
            for n in &names {
                sources.remove(n);
            }
            names
        };
        let mut h = self.health.write().await;
        for n in removed {
            h.forget(&n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::health::HealthState;
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
        // Health entry should also be cleaned up
        assert!(!reg.health_snapshot().await.contains_key("alpha"));
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
        let h = reg.health_snapshot().await;
        assert!(!h.keys().any(|k| k.starts_with("device:")));
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

    #[tokio::test]
    async fn test_poll_records_success_in_health() {
        let reg = PerceptionRegistry::new(AggregationStrategy::Latest, 10);
        reg.register_source(Arc::new(MockPerceptionSource::new("alive"))).await;
        reg.poll_all().await;
        let snap = reg.health_snapshot().await;
        assert_eq!(snap["alive"].success_count, 1);
        assert_eq!(snap["alive"].state, HealthState::Healthy);
    }

    #[tokio::test]
    async fn test_poll_records_timeout_for_slow_source() {
        // Use a slow source with a tiny custom timeout
        let cfg = HealthConfig {
            default_timeout: std::time::Duration::from_millis(20),
            default_interval: std::time::Duration::from_millis(100),
            ..Default::default()
        };
        let reg = PerceptionRegistry::with_health_config(AggregationStrategy::Latest, 10, cfg);
        reg.register_source(Arc::new(
            MockPerceptionSource::new("slow")
                .with_observe_delay(std::time::Duration::from_millis(200)),
        ))
        .await;
        reg.poll_all().await;
        let snap = reg.health_snapshot().await;
        assert_eq!(snap["slow"].failure_count, 1);
        assert!(snap["slow"].consecutive_failures >= 1);
    }

    #[tokio::test]
    async fn test_quarantined_source_is_skipped_until_probe_succeeds() {
        let cfg = HealthConfig {
            default_timeout: std::time::Duration::from_millis(20),
            default_interval: std::time::Duration::from_millis(50),
            max_timeout: std::time::Duration::from_millis(50),
            max_interval: std::time::Duration::from_millis(200),
            degrade_threshold: 1,
            quarantine_threshold: 2,
        };
        let reg = PerceptionRegistry::with_health_config(AggregationStrategy::Latest, 10, cfg);
        let mock = Arc::new(
            MockPerceptionSource::new("flaky")
                .with_observe_delay(std::time::Duration::from_millis(200)),
        );
        reg.register_source(mock.clone()).await;
        // 2 timeouts → quarantined.
        reg.poll_all().await;
        reg.poll_all().await;
        let snap = reg.health_snapshot().await;
        assert_eq!(snap["flaky"].state, HealthState::Quarantined);

        // Subsequent poll_all should skip the quarantined source — failure count stays.
        let prev_failures = snap["flaky"].failure_count;
        reg.poll_all().await;
        let snap = reg.health_snapshot().await;
        assert_eq!(
            snap["flaky"].failure_count, prev_failures,
            "quarantined sources should not be polled by poll_all"
        );
    }

    #[tokio::test]
    async fn test_poll_all_appends_to_observation_store() {
        use crate::perception::persistence::JsonlObservationStore;

        let dir = std::env::temp_dir().join(format!(
            "syscity-registry-store-{}",
            std::process::id()
        ));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let store = Arc::new(JsonlObservationStore::open(&dir).await.unwrap());

        let reg = PerceptionRegistry::new(AggregationStrategy::Latest, 10)
            .with_store(store.clone());
        reg.register_source(Arc::new(
            MockPerceptionSource::new("persisted")
                .with_modality(Modality::System)
                .with_data(serde_json::json!({"v": 1})),
        ))
        .await;
        reg.poll_all().await;

        // Read back through the store directly.
        let q = PerceptionQuery::default();
        let persisted = store.query(&q, None).await.unwrap();
        assert!(
            !persisted.is_empty(),
            "expected at least one observation to be persisted"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
