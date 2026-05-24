//! Memory Effectiveness Tracking
//!
//! Tracks how often recalled memories are actually useful ("hit rate")
//! and adjusts memory weights/tiers accordingly.
//!
//! Metrics:
//! - Recall hit rate: was the memory referenced in the LLM response?
//! - Type effectiveness: which memory types are most useful?
//! - Auto-weight adjustment: boost high-performing memories, demote low-performing ones.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;
use tokio::sync::RwLock;

/// A single recall event tracked for effectiveness analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallEvent {
    /// Unique recall ID.
    pub recall_id: String,
    /// Memory ID that was recalled.
    pub memory_id: String,
    /// Session key where recall happened.
    pub session_key: String,
    /// When the recall occurred.
    pub recalled_at: SystemTime,
    /// Whether the memory was "hit" (used/referenced in response).
    pub hit: bool,
    /// Memory type at time of recall.
    pub memory_type: String,
    /// Importance score at time of recall.
    pub importance_score: f32,
    /// Position in the retrieved results (0 = top).
    pub rank: usize,
}

/// Aggregated statistics for a memory or type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EffectivenessStats {
    /// Total number of recalls.
    pub total_recalls: u64,
    /// Number of hits.
    pub hits: u64,
    /// Hit rate (hits / total_recalls).
    pub hit_rate: f32,
    /// Average rank when recalled.
    pub avg_rank: f32,
    /// Times promoted due to effectiveness.
    pub promotions: u32,
    /// Times demoted due to ineffectiveness.
    pub demotions: u32,
}

impl EffectivenessStats {
    /// Update stats with a new recall event.
    pub fn record(&mut self, hit: bool, rank: usize) {
        self.total_recalls += 1;
        if hit {
            self.hits += 1;
        }
        self.hit_rate = self.hits as f32 / self.total_recalls as f32;
        // Update rolling average rank
        let total = self.total_recalls as f32;
        self.avg_rank = (self.avg_rank * (total - 1.0) + rank as f32) / total;
    }
}

/// Effectiveness tracker configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectivenessConfig {
    /// Whether auto-adjustment is enabled.
    pub auto_adjust: bool,
    /// Hit rate threshold for promotion (e.g., 0.7 = 70%).
    pub promotion_threshold: f32,
    /// Hit rate threshold for demotion (e.g., 0.2 = 20%).
    pub demotion_threshold: f32,
    /// Minimum recalls before considering adjustment.
    pub min_recalls_for_adjustment: u64,
    /// Boost amount to apply to importance score on promotion.
    pub importance_boost: f32,
    /// Penalty amount to apply to importance score on demotion.
    pub importance_penalty: f32,
    /// Maximum importance score cap.
    pub max_importance: f32,
    /// Minimum importance score floor.
    pub min_importance: f32,
}

impl Default for EffectivenessConfig {
    fn default() -> Self {
        Self {
            auto_adjust: true,
            promotion_threshold: 0.7,
            demotion_threshold: 0.2,
            min_recalls_for_adjustment: 3,
            importance_boost: 0.1,
            importance_penalty: 0.1,
            max_importance: 1.0,
            min_importance: 0.0,
        }
    }
}

/// Action recommended by the effectiveness evaluator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectivenessAction {
    /// Increase importance and consider promotion.
    Boost,
    /// Decrease importance and consider demotion.
    Penalize,
    /// No action needed.
    NoOp,
}

/// Tracks memory recall effectiveness and recommends adjustments.
pub struct EffectivenessTracker {
    config: EffectivenessConfig,
    /// memory_id -> recall events
    events: RwLock<HashMap<String, Vec<RecallEvent>>>,
    /// memory_type -> aggregated stats
    type_stats: RwLock<HashMap<String, EffectivenessStats>>,
    /// memory_id -> aggregated stats (computed on demand)
    memory_stats_cache: RwLock<HashMap<String, EffectivenessStats>>,
}

impl EffectivenessTracker {
    /// Create a new effectiveness tracker.
    pub fn new(config: EffectivenessConfig) -> Self {
        Self {
            config,
            events: RwLock::new(HashMap::new()),
            type_stats: RwLock::new(HashMap::new()),
            memory_stats_cache: RwLock::new(HashMap::new()),
        }
    }

    /// Record a recall event.
    pub async fn record_recall(
        &self,
        recall_id: impl Into<String>,
        memory_id: impl Into<String>,
        session_key: impl Into<String>,
        memory_type: impl Into<String>,
        importance_score: f32,
        rank: usize,
    ) {
        let recall_id = recall_id.into();
        let memory_id = memory_id.into();
        let session_key = session_key.into();
        let memory_type = memory_type.into();

        let event = RecallEvent {
            recall_id,
            memory_id: memory_id.clone(),
            session_key,
            recalled_at: SystemTime::now(),
            hit: false, // Will be updated later when response is evaluated
            memory_type: memory_type.clone(),
            importance_score,
            rank,
        };

        // Store event
        let mut events_guard = self.events.write().await;
        events_guard
            .entry(memory_id.clone())
            .or_default()
            .push(event);
        drop(events_guard);

        // Invalidate cache for this memory
        let mut cache_guard = self.memory_stats_cache.write().await;
        cache_guard.remove(&memory_id);
        drop(cache_guard);
    }

    /// Mark a recall as a "hit" (the memory was useful in the response).
    pub async fn mark_hit(&self, recall_id: impl AsRef<str>) {
        let recall_id = recall_id.as_ref();
        let mut events_guard = self.events.write().await;
        for events in events_guard.values_mut() {
            for event in events.iter_mut() {
                if event.recall_id == recall_id {
                    event.hit = true;
                    break;
                }
            }
        }
        // Clear cache since stats changed
        drop(events_guard);
        let mut cache_guard = self.memory_stats_cache.write().await;
        cache_guard.clear();
    }

    /// Get stats for a specific memory.
    pub async fn memory_stats(&self, memory_id: &str) -> Option<EffectivenessStats> {
        // Check cache first
        {
            let cache = self.memory_stats_cache.read().await;
            if let Some(stats) = cache.get(memory_id) {
                return Some(stats.clone());
            }
        }

        let events_guard = self.events.read().await;
        let events = events_guard.get(memory_id)?;

        let mut stats = EffectivenessStats::default();
        for event in events {
            stats.record(event.hit, event.rank);
        }
        drop(events_guard);

        // Update cache
        let mut cache = self.memory_stats_cache.write().await;
        cache.insert(memory_id.to_string(), stats.clone());

        Some(stats)
    }

    /// Get aggregated stats for a memory type.
    pub async fn type_stats(&self, memory_type: &str) -> EffectivenessStats {
        // Recompute from events
        let events_guard = self.events.read().await;
        let mut stats = EffectivenessStats::default();
        for events in events_guard.values() {
            for event in events {
                if event.memory_type == memory_type {
                    stats.record(event.hit, event.rank);
                }
            }
        }
        drop(events_guard);

        let mut type_guard = self.type_stats.write().await;
        type_guard.insert(memory_type.to_string(), stats.clone());
        stats
    }

    /// Evaluate a memory and recommend an action.
    pub async fn evaluate(&self, memory_id: &str, current_importance: f32) -> EffectivenessAction {
        if !self.config.auto_adjust {
            return EffectivenessAction::NoOp;
        }

        let Some(stats) = self.memory_stats(memory_id).await else {
            return EffectivenessAction::NoOp;
        };

        if stats.total_recalls < self.config.min_recalls_for_adjustment {
            return EffectivenessAction::NoOp;
        }

        if stats.hit_rate >= self.config.promotion_threshold {
            if current_importance < self.config.max_importance {
                return EffectivenessAction::Boost;
            }
        }

        if stats.hit_rate <= self.config.demotion_threshold {
            if current_importance > self.config.min_importance {
                return EffectivenessAction::Penalize;
            }
        }

        EffectivenessAction::NoOp
    }

    /// Apply the recommended action to an importance score.
    pub fn apply_action(&self, action: EffectivenessAction, current_importance: f32) -> f32 {
        match action {
            EffectivenessAction::Boost => {
                (current_importance + self.config.importance_boost).min(self.config.max_importance)
            }
            EffectivenessAction::Penalize => (current_importance - self.config.importance_penalty)
                .max(self.config.min_importance),
            EffectivenessAction::NoOp => current_importance,
        }
    }

    /// Get overall system-wide hit rate.
    pub async fn overall_hit_rate(&self) -> f32 {
        let events_guard = self.events.read().await;
        let total: u64 = events_guard.values().map(|v| v.len() as u64).sum();
        if total == 0 {
            return 0.0;
        }
        let hits: u64 = events_guard
            .values()
            .flat_map(|v| v.iter())
            .filter(|e| e.hit)
            .count() as u64;
        hits as f32 / total as f32
    }

    /// Get top-performing memory IDs by hit rate.
    pub async fn top_performers(&self, limit: usize) -> Vec<(String, EffectivenessStats)> {
        let events_guard = self.events.read().await;
        let mut scored: Vec<(String, EffectivenessStats)> = events_guard
            .keys()
            .filter_map(|id| {
                let mut stats = EffectivenessStats::default();
                if let Some(events) = events_guard.get(id) {
                    for event in events {
                        stats.record(event.hit, event.rank);
                    }
                }
                if stats.total_recalls >= self.config.min_recalls_for_adjustment {
                    Some((id.clone(), stats))
                } else {
                    None
                }
            })
            .collect();
        drop(events_guard);

        scored.sort_by(|a, b| b.1.hit_rate.partial_cmp(&a.1.hit_rate).unwrap());
        scored.into_iter().take(limit).collect()
    }

    /// Get under-performing memory IDs by hit rate.
    pub async fn under_performers(&self, limit: usize) -> Vec<(String, EffectivenessStats)> {
        let events_guard = self.events.read().await;
        let mut scored: Vec<(String, EffectivenessStats)> = events_guard
            .keys()
            .filter_map(|id| {
                let mut stats = EffectivenessStats::default();
                if let Some(events) = events_guard.get(id) {
                    for event in events {
                        stats.record(event.hit, event.rank);
                    }
                }
                if stats.total_recalls >= self.config.min_recalls_for_adjustment {
                    Some((id.clone(), stats))
                } else {
                    None
                }
            })
            .collect();
        drop(events_guard);

        scored.sort_by(|a, b| a.1.hit_rate.partial_cmp(&b.1.hit_rate).unwrap());
        scored.into_iter().take(limit).collect()
    }

    /// Total number of tracked recall events.
    pub async fn total_events(&self) -> usize {
        let events_guard = self.events.read().await;
        events_guard.values().map(|v| v.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_record_recall_and_hit() {
        let tracker = EffectivenessTracker::new(EffectivenessConfig::default());

        tracker
            .record_recall("r1", "m1", "session:1", "fact", 0.5, 0)
            .await;
        tracker
            .record_recall("r2", "m1", "session:2", "fact", 0.5, 1)
            .await;
        tracker
            .record_recall("r3", "m2", "session:1", "preference", 0.8, 0)
            .await;

        // Mark r1 and r3 as hits
        tracker.mark_hit("r1").await;
        tracker.mark_hit("r3").await;

        let m1_stats = tracker.memory_stats("m1").await.unwrap();
        assert_eq!(m1_stats.total_recalls, 2);
        assert_eq!(m1_stats.hits, 1);
        assert_eq!(m1_stats.hit_rate, 0.5);

        let m2_stats = tracker.memory_stats("m2").await.unwrap();
        assert_eq!(m2_stats.total_recalls, 1);
        assert_eq!(m2_stats.hits, 1);
        assert_eq!(m2_stats.hit_rate, 1.0);

        let overall = tracker.overall_hit_rate().await;
        assert_eq!(overall, 2.0 / 3.0);
    }

    #[tokio::test]
    async fn test_effectiveness_evaluation() {
        let config = EffectivenessConfig {
            auto_adjust: true,
            promotion_threshold: 0.7,
            demotion_threshold: 0.2,
            min_recalls_for_adjustment: 3,
            importance_boost: 0.1,
            importance_penalty: 0.1,
            max_importance: 1.0,
            min_importance: 0.0,
        };
        let tracker = EffectivenessTracker::new(config);

        // Memory with high hit rate (3/3)
        for i in 0..3 {
            tracker
                .record_recall(format!("r{}", i), "m_high", "session:1", "fact", 0.6, 0)
                .await;
            tracker.mark_hit(format!("r{}", i)).await;
        }

        let action = tracker.evaluate("m_high", 0.6).await;
        assert_eq!(action, EffectivenessAction::Boost);

        let new_importance = tracker.apply_action(action, 0.6);
        assert!((new_importance - 0.7).abs() < 0.001);

        // Memory with low hit rate (0/3)
        for i in 3..6 {
            tracker
                .record_recall(format!("r{}", i), "m_low", "session:1", "fact", 0.6, 0)
                .await;
        }

        let action = tracker.evaluate("m_low", 0.6).await;
        assert_eq!(action, EffectivenessAction::Penalize);

        let new_importance = tracker.apply_action(action, 0.6);
        assert!((new_importance - 0.5).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_top_and_under_performers() {
        let tracker = EffectivenessTracker::new(EffectivenessConfig {
            min_recalls_for_adjustment: 2,
            ..Default::default()
        });

        // m1: 3/3 hits
        for i in 0..3 {
            tracker
                .record_recall(format!("r1-{}", i), "m1", "s1", "fact", 0.5, 0)
                .await;
            tracker.mark_hit(format!("r1-{}", i)).await;
        }

        // m2: 0/3 hits
        for i in 0..3 {
            tracker
                .record_recall(format!("r2-{}", i), "m2", "s1", "fact", 0.5, 0)
                .await;
        }

        let top = tracker.top_performers(10).await;
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, "m1");
        assert_eq!(top[0].1.hit_rate, 1.0);

        let under = tracker.under_performers(10).await;
        assert_eq!(under[0].0, "m2");
        assert_eq!(under[0].1.hit_rate, 0.0);
    }
}
