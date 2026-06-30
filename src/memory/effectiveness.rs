//! Memory Effectiveness Tracking
//!
//! Tracks how often recalled memories are actually useful ("hit rate")
//! and adjusts memory weights/tiers accordingly.
//!
//! Metrics:
//! - Recall hit rate: was the memory referenced in the LLM response?
//! - Type effectiveness: which memory types are most useful?
//! - Auto-weight adjustment: boost high-performing memories, demote
//!   low-performing ones.

use std::collections::HashMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::debug;

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
    /// Hit rate threshold for direct tier promotion (e.g., 0.9 = 90%).
    pub promote_directly_threshold: f32,
    /// Hit rate threshold for direct tier demotion (e.g., 0.1 = 10%).
    pub demote_directly_threshold: f32,
    /// Maximum recall events to retain per memory ID before pruning oldest.
    /// Prevents unbounded HashMap growth in the effectiveness tracker.
    #[serde(default = "default_max_events_per_memory")]
    pub max_events_per_memory: usize,
    /// Maximum number of distinct memory IDs to track across all internal maps.
    /// When exceeded, the least recently accessed memories are pruned.
    #[serde(default = "default_max_tracked_memories")]
    pub max_tracked_memories: usize,
}

fn default_max_events_per_memory() -> usize {
    1000
}

fn default_max_tracked_memories() -> usize {
    50_000
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
            promote_directly_threshold: 0.9,
            demote_directly_threshold: 0.1,
            max_events_per_memory: 1000,
            max_tracked_memories: 50_000,
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
    /// recall_id -> (memory_id, index in the events vec)
    recall_index: RwLock<HashMap<String, (String, usize)>>,
    /// memory_type -> aggregated stats
    type_stats: RwLock<HashMap<String, EffectivenessStats>>,
    /// memory_id -> aggregated stats (computed on demand)
    memory_stats_cache: RwLock<HashMap<String, EffectivenessStats>>,
    /// memory_id -> (promotions, demotions) counters for tier migrations.
    promotion_counters: RwLock<HashMap<String, (u32, u32)>>,
}

impl EffectivenessTracker {
    /// Create a new effectiveness tracker.
    pub fn new(config: EffectivenessConfig) -> Self {
        Self {
            config,
            events: RwLock::new(HashMap::new()),
            recall_index: RwLock::new(HashMap::new()),
            type_stats: RwLock::new(HashMap::new()),
            memory_stats_cache: RwLock::new(HashMap::new()),
            promotion_counters: RwLock::new(HashMap::new()),
        }
    }

    /// Get the tracker configuration.
    pub fn config(&self) -> &EffectivenessConfig {
        &self.config
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
            recall_id: recall_id.clone(),
            memory_id: memory_id.clone(),
            session_key,
            recalled_at: SystemTime::now(),
            hit: false, // Will be updated later when response is evaluated
            memory_type: memory_type.clone(),
            importance_score,
            rank,
        };

        // Acquire events lock, push, prune oldest if over cap.
        let (final_index, pruned_ids, excess) = {
            let mut events_guard = self.events.write().await;
            let events = events_guard.entry(memory_id.clone()).or_default();
            events.push(event);

            let mut pruned_ids = Vec::new();
            let excess = if events.len() > self.config.max_events_per_memory {
                let excess = events.len() - self.config.max_events_per_memory;
                let removed: Vec<RecallEvent> = events.drain(..excess).collect();
                pruned_ids = removed.into_iter().map(|e| e.recall_id).collect();
                excess
            } else {
                0
            };

            // After pruning from the front, the new event is always the last element.
            let final_index = events.len() - 1;
            (final_index, pruned_ids, excess)
            // events_guard dropped here
        };

        // Remove pruned entries from recall_index and fix existing indices
        // that shifted due to front-pruning.
        {
            let mut index_guard = self.recall_index.write().await;
            for rid in &pruned_ids {
                index_guard.remove(rid);
            }
            // Update existing entries: front-pruning shifted all remaining events
            // by `excess` positions toward index 0.
            if excess > 0 {
                for (_, (mem_id, idx)) in index_guard.iter_mut() {
                    if *mem_id == memory_id {
                        *idx = idx.saturating_sub(excess);
                    }
                }
            }
            index_guard.insert(recall_id, (memory_id.clone(), final_index));
        }

        // Invalidate stats cache for this memory
        {
            let mut cache_guard = self.memory_stats_cache.write().await;
            cache_guard.remove(&memory_id);
        }

        // Enforce total tracked memories limit (lazy: only checks periodically)
        {
            let events_guard = self.events.read().await;
            if events_guard.len() > self.config.max_tracked_memories {
                drop(events_guard);
                self.prune_overflow_memories().await;
            }
        }
    }

    /// Mark a recall as a "hit" (the memory was useful in the response).
    pub async fn mark_hit(&self, recall_id: impl AsRef<str>) {
        let recall_id = recall_id.as_ref();

        // Use O(1) index lookup
        let (memory_id, index) = {
            let index_guard = self.recall_index.read().await;
            match index_guard.get(recall_id) {
                Some(entry) => entry.clone(),
                None => return, // Recall ID not found; skip
            }
        };

        // Update the specific event directly
        let mut events_guard = self.events.write().await;
        if let Some(events) = events_guard.get_mut(&memory_id) {
            if let Some(event) = events.get_mut(index) {
                event.hit = true;
            }
        }
        drop(events_guard);

        // Invalidate cache for this memory
        let mut cache_guard = self.memory_stats_cache.write().await;
        cache_guard.remove(&memory_id);
    }

    /// Get stats for a specific memory.
    pub async fn memory_stats(&self, memory_id: &str) -> Option<EffectivenessStats> {
        // Check cache first, but always merge live counters because they may
        // have changed since the cached value was written.
        {
            let cache = self.memory_stats_cache.read().await;
            if let Some(mut stats) = cache.get(memory_id).cloned() {
                let counters = self.promotion_counters.read().await;
                if let Some(&(promotions, demotions)) = counters.get(memory_id) {
                    stats.promotions = promotions;
                    stats.demotions = demotions;
                }
                return Some(stats);
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

        // Merge tier migration counters into stats.
        {
            let counters = self.promotion_counters.read().await;
            if let Some(&(promotions, demotions)) = counters.get(memory_id) {
                stats.promotions = promotions;
                stats.demotions = demotions;
            }
        }

        cache.insert(memory_id.to_string(), stats.clone());

        Some(stats)
    }

    /// Record a tier promotion for a memory.
    pub async fn record_promotion(&self, memory_id: impl Into<String>) {
        let memory_id = memory_id.into();
        let mut counters = self.promotion_counters.write().await;
        counters.entry(memory_id).or_insert((0, 0)).0 += 1;
    }

    /// Record a tier demotion for a memory.
    pub async fn record_demotion(&self, memory_id: impl Into<String>) {
        let memory_id = memory_id.into();
        let mut counters = self.promotion_counters.write().await;
        counters.entry(memory_id).or_insert((0, 0)).1 += 1;
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

        if stats.hit_rate >= self.config.promotion_threshold
            && current_importance < self.config.max_importance
        {
            return EffectivenessAction::Boost;
        }

        if stats.hit_rate <= self.config.demotion_threshold
            && current_importance > self.config.min_importance
        {
            return EffectivenessAction::Penalize;
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

        scored.sort_by(|a, b| {
            b.1.hit_rate
                .partial_cmp(&a.1.hit_rate)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
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

        scored.sort_by(|a, b| {
            a.1.hit_rate
                .partial_cmp(&b.1.hit_rate)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.into_iter().take(limit).collect()
    }

    /// Total number of tracked recall events.
    pub async fn total_events(&self) -> usize {
        let events_guard = self.events.read().await;
        events_guard.values().map(|v| v.len()).sum()
    }

    /// Prune the least-recently-referenced memories when the total tracked
    /// count exceeds `max_tracked_memories`. Removes entire memory entries
    /// (events, recall_index, stats cache, promotion counters) for the
    /// memories with the fewest events until under the limit.
    async fn prune_overflow_memories(&self) {
        let target = self.config.max_tracked_memories / 2; // prune down to 50%

        let victims: Vec<String> = {
            let events_guard = self.events.read().await;
            if events_guard.len() <= target {
                return;
            }
            let mut by_count: Vec<(String, usize)> = events_guard
                .iter()
                .map(|(id, evts)| (id.clone(), evts.len()))
                .collect();
            drop(events_guard);

            by_count.sort_by_key(|(_, count)| *count);

            let to_remove = by_count.len() - target;
            by_count
                .into_iter()
                .take(to_remove)
                .map(|(id, _)| id)
                .collect()
        };

        if victims.is_empty() {
            return;
        }

        let victim_count = victims.len();

        // Remove events
        {
            let mut events_guard = self.events.write().await;
            for id in &victims {
                // Collect recall_ids to remove from recall_index
                if let Some(evts) = events_guard.remove(id) {
                    let mut index_guard = self.recall_index.write().await;
                    for evt in evts {
                        index_guard.remove(&evt.recall_id);
                    }
                }
            }
        }

        // Remove from stats cache and promotion counters
        {
            let mut cache_guard = self.memory_stats_cache.write().await;
            let mut counters_guard = self.promotion_counters.write().await;
            for id in &victims {
                cache_guard.remove(id);
                counters_guard.remove(id);
            }
        }

        debug!("Pruned {} overflow memory entries from effectiveness tracker", victim_count);
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
            promote_directly_threshold: 0.9,
            demote_directly_threshold: 0.1,
            max_events_per_memory: 1000,
            max_tracked_memories: 50_000,
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
