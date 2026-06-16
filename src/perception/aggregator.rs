//! Temporal observation aggregation.
//!
//! The [`TemporalAggregator`] maintains a sliding window of recent observations
//! and applies a configurable [`AggregationStrategy`] to produce stable entities.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::perception::scene_graph::Entity;
use crate::perception::Observation;

/// Strategy for converting a window of observations into stable entities.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AggregationStrategy {
    /// Latest observation wins (simple passthrough).
    Latest,
    /// Entity exists if it appears in more than half the observations.
    Majority,
    /// Entity exists if seen at least `N` times in the window.
    CountThreshold(usize),
    /// Entity exists if its confidence-weighted sum exceeds the threshold.
    ConfidenceWeighted(f32),
}

impl Default for AggregationStrategy {
    fn default() -> Self {
        Self::Latest
    }
}

/// Sliding-window temporal aggregator.
pub struct TemporalAggregator {
    strategy: AggregationStrategy,
    window_size: Duration,
    observations: VecDeque<(Instant, Observation)>,
}

impl TemporalAggregator {
    /// Create a new aggregator with the given strategy and window size.
    pub fn new(strategy: AggregationStrategy, window_size: Duration) -> Self {
        Self {
            strategy,
            window_size,
            observations: VecDeque::new(),
        }
    }

    /// Push a new observation into the sliding window.
    pub fn push(&mut self, obs: Observation) {
        self.observations.push_back((Instant::now(), obs));
        self.prune();
    }

    /// Remove expired observations from the window.
    pub fn prune(&mut self) {
        let cutoff = Instant::now() - self.window_size;
        while let Some(front) = self.observations.front() {
            if front.0 < cutoff {
                self.observations.pop_front();
            } else {
                break;
            }
        }
    }

    /// Aggregate the current window into stable entities.
    pub fn aggregate(&self) -> Vec<Entity> {
        match self.strategy {
            AggregationStrategy::Latest => self.aggregate_latest(),
            AggregationStrategy::Majority => self.aggregate_majority(),
            AggregationStrategy::CountThreshold(n) => self.aggregate_count_threshold(n),
            AggregationStrategy::ConfidenceWeighted(t) => self.aggregate_confidence_weighted(t),
        }
    }

    /// Return the number of observations in the current window.
    pub fn len(&self) -> usize {
        self.observations.len()
    }

    /// Return `true` if the window is empty.
    pub fn is_empty(&self) -> bool {
        self.observations.is_empty()
    }

    // ── Private strategy implementations ────────────────────────────────

    fn aggregate_latest(&self) -> Vec<Entity> {
        // Take the last observation per source name
        let mut latest: std::collections::HashMap<&str, &Observation> =
            std::collections::HashMap::new();
        for (_, obs) in &self.observations {
            latest.insert(obs.source.as_str(), obs);
        }
        latest
            .values()
            .map(|obs| Entity {
                id: crate::perception::EntityId::new(obs.source.clone()),
                label: format!("{:?}", obs.modality),
                modality: obs.modality,
                first_seen: obs.timestamp,
                last_seen: obs.timestamp,
                confidence: obs.confidence,
                properties: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("data".to_string(), obs.data.clone());
                    m
                },
                spatial: obs.spatial.clone(),
                relationships: Vec::new(),
            })
            .collect()
    }

    fn aggregate_majority(&self) -> Vec<Entity> {
        let mut counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        let total = self.observations.len();
        for (_, obs) in &self.observations {
            *counts.entry(obs.source.as_str()).or_insert(0) += 1;
        }
        let threshold = total / 2;
        let sources: Vec<&str> = counts
            .into_iter()
            .filter(|(_, c)| *c > threshold)
            .map(|(s, _)| s)
            .collect();

        self.observations
            .iter()
            .rev()
            .filter(|(_, obs)| sources.contains(&obs.source.as_str()))
            .fold(std::collections::HashMap::new(), |mut acc, (_, obs)| {
                acc.entry(obs.source.as_str()).or_insert(obs);
                acc
            })
            .into_values()
            .map(|obs| Entity {
                id: crate::perception::EntityId::new(obs.source.clone()),
                label: format!("{:?}", obs.modality),
                modality: obs.modality,
                first_seen: obs.timestamp,
                last_seen: obs.timestamp,
                confidence: obs.confidence,
                properties: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("data".to_string(), obs.data.clone());
                    m
                },
                spatial: obs.spatial.clone(),
                relationships: Vec::new(),
            })
            .collect()
    }

    fn aggregate_count_threshold(&self, threshold: usize) -> Vec<Entity> {
        let mut counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for (_, obs) in &self.observations {
            *counts.entry(obs.source.as_str()).or_insert(0) += 1;
        }
        let sources: Vec<&str> = counts
            .into_iter()
            .filter(|(_, c)| *c >= threshold)
            .map(|(s, _)| s)
            .collect();

        self.observations
            .iter()
            .rev()
            .filter(|(_, obs)| sources.contains(&obs.source.as_str()))
            .fold(std::collections::HashMap::new(), |mut acc, (_, obs)| {
                acc.entry(obs.source.as_str()).or_insert(obs);
                acc
            })
            .into_values()
            .map(|obs| Entity {
                id: crate::perception::EntityId::new(obs.source.clone()),
                label: format!("{:?}", obs.modality),
                modality: obs.modality,
                first_seen: obs.timestamp,
                last_seen: obs.timestamp,
                confidence: obs.confidence,
                properties: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("data".to_string(), obs.data.clone());
                    m
                },
                spatial: obs.spatial.clone(),
                relationships: Vec::new(),
            })
            .collect()
    }

    fn aggregate_confidence_weighted(&self, threshold: f32) -> Vec<Entity> {
        let mut scores: std::collections::HashMap<&str, f32> =
            std::collections::HashMap::new();
        for (_, obs) in &self.observations {
            *scores.entry(obs.source.as_str()).or_insert(0.0) += obs.confidence;
        }
        let sources: Vec<&str> = scores
            .into_iter()
            .filter(|(_, s)| *s >= threshold)
            .map(|(s, _)| s)
            .collect();

        self.observations
            .iter()
            .rev()
            .filter(|(_, obs)| sources.contains(&obs.source.as_str()))
            .fold(std::collections::HashMap::new(), |mut acc, (_, obs)| {
                acc.entry(obs.source.as_str()).or_insert(obs);
                acc
            })
            .into_values()
            .map(|obs| Entity {
                id: crate::perception::EntityId::new(obs.source.clone()),
                label: format!("{:?}", obs.modality),
                modality: obs.modality,
                first_seen: obs.timestamp,
                last_seen: obs.timestamp,
                confidence: obs.confidence,
                properties: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("data".to_string(), obs.data.clone());
                    m
                },
                spatial: obs.spatial.clone(),
                relationships: Vec::new(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::{Modality, ObservationId, SpatialContext};

    fn make_obs(source: &str) -> Observation {
        Observation {
            id: ObservationId::new(),
            source: source.to_string(),
            modality: Modality::System,
            timestamp: Instant::now(),
            confidence: 1.0,
            spatial: None,
            data: serde_json::json!({"value": 42}),
        }
    }

    #[test]
    fn test_empty_window() {
        let agg = TemporalAggregator::new(AggregationStrategy::Latest, Duration::from_secs(10));
        assert!(agg.is_empty());
        assert!(agg.aggregate().is_empty());
    }

    #[test]
    fn test_latest_strategy() {
        let mut agg = TemporalAggregator::new(AggregationStrategy::Latest, Duration::from_secs(10));
        agg.push(make_obs("sensor_a"));
        std::thread::sleep(std::time::Duration::from_millis(2));
        agg.push(make_obs("sensor_b"));

        let entities = agg.aggregate();
        assert_eq!(entities.len(), 2);
    }

    #[test]
    fn test_count_threshold() {
        let mut agg =
            TemporalAggregator::new(AggregationStrategy::CountThreshold(3), Duration::from_secs(10));
        for _ in 0..5 {
            agg.push(make_obs("frequent"));
        }
        agg.push(make_obs("rare"));

        let entities = agg.aggregate();
        let names: Vec<String> = entities.iter().map(|e| e.id.to_string()).collect();
        assert!(names.contains(&"frequent".to_string()));
        assert!(!names.contains(&"rare".to_string()));
    }

    #[test]
    fn test_window_pruning() {
        let mut agg =
            TemporalAggregator::new(AggregationStrategy::Latest, Duration::from_millis(10));
        agg.push(make_obs("a"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        // After sleeping 20ms, the first observation should be pruned on next push
        agg.push(make_obs("b"));
        assert_eq!(agg.len(), 1);
    }
}
