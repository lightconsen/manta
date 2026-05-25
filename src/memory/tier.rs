//! Memory Tier System
//!
//! Manages four memory tiers with distinct retention policies:
//! - Working: active context (agent's current view)
//! - ShortTerm: recent session history (hours to days)
//! - LongTerm: consolidated semantic memories (weeks to months)
//! - Archival: cold storage (months to years)
//!
//! Dreams promote memories between tiers based on importance, access patterns,
//! and age. Each tier has capacity limits and retention rules.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

/// Memory tier levels, ordered from most to least ephemeral.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
    /// Active working context — highest volatility, no persistence.
    Working,
    /// Recent session history — hours to days retention.
    ShortTerm,
    /// Consolidated semantic memories — weeks to months retention.
    LongTerm,
    /// Cold archival storage — months to years retention.
    Archival,
}

impl MemoryTier {
    /// Return the next higher tier (more persistent).
    pub fn promote(self) -> Option<Self> {
        match self {
            MemoryTier::Working => Some(MemoryTier::ShortTerm),
            MemoryTier::ShortTerm => Some(MemoryTier::LongTerm),
            MemoryTier::LongTerm => Some(MemoryTier::Archival),
            MemoryTier::Archival => None,
        }
    }

    /// Return the next lower tier (more ephemeral).
    pub fn demote(self) -> Option<Self> {
        match self {
            MemoryTier::Archival => Some(MemoryTier::LongTerm),
            MemoryTier::LongTerm => Some(MemoryTier::ShortTerm),
            MemoryTier::ShortTerm => Some(MemoryTier::Working),
            MemoryTier::Working => None,
        }
    }

    /// Default retention duration for this tier in seconds.
    pub fn default_ttl_secs(&self) -> u64 {
        match self {
            MemoryTier::Working => 60 * 60,             // 1 hour
            MemoryTier::ShortTerm => 7 * 24 * 60 * 60,  // 7 days
            MemoryTier::LongTerm => 90 * 24 * 60 * 60,  // 90 days
            MemoryTier::Archival => 365 * 24 * 60 * 60, // 1 year
        }
    }

    /// Maximum number of memories this tier should hold.
    pub fn default_capacity(&self) -> usize {
        match self {
            MemoryTier::Working => 50,
            MemoryTier::ShortTerm => 500,
            MemoryTier::LongTerm => 5_000,
            MemoryTier::Archival => 50_000,
        }
    }

    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            MemoryTier::Working => "working",
            MemoryTier::ShortTerm => "short_term",
            MemoryTier::LongTerm => "long_term",
            MemoryTier::Archival => "archival",
        }
    }

    /// Parse a tier from its label string.
    pub fn from_label(s: &str) -> crate::Result<Self> {
        match s {
            "working" => Ok(MemoryTier::Working),
            "short_term" => Ok(MemoryTier::ShortTerm),
            "long_term" => Ok(MemoryTier::LongTerm),
            "archival" => Ok(MemoryTier::Archival),
            _ => Err(crate::error::MantaError::Validation(format!(
                "Unknown memory tier: {}",
                s
            ))),
        }
    }
}

impl std::fmt::Display for MemoryTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Configuration for each tier's retention policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    /// Whether this tier is enabled.
    pub enabled: bool,
    /// Maximum capacity before eviction.
    pub capacity: usize,
    /// Default TTL in seconds (0 = no TTL).
    pub ttl_secs: u64,
    /// Minimum importance score to enter this tier.
    pub min_importance: f32,
    /// Minimum access count to promote from lower tier.
    pub min_access_count: u32,
}

impl Default for TierConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            capacity: 1000,
            ttl_secs: 7 * 24 * 60 * 60,
            min_importance: 0.3,
            min_access_count: 1,
        }
    }
}

/// Full tier system configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierSystemConfig {
    /// Configuration per tier.
    pub tiers: HashMap<MemoryTier, TierConfig>,
    /// Whether automatic promotion/demotion is enabled.
    pub auto_promote: bool,
    /// Interval in seconds between tier maintenance scans.
    pub maintenance_interval_secs: u64,
}

impl Default for TierSystemConfig {
    fn default() -> Self {
        let mut tiers = HashMap::new();
        tiers.insert(
            MemoryTier::Working,
            TierConfig {
                enabled: true,
                capacity: MemoryTier::Working.default_capacity(),
                ttl_secs: MemoryTier::Working.default_ttl_secs(),
                min_importance: 0.0,
                min_access_count: 0,
            },
        );
        tiers.insert(
            MemoryTier::ShortTerm,
            TierConfig {
                enabled: true,
                capacity: MemoryTier::ShortTerm.default_capacity(),
                ttl_secs: MemoryTier::ShortTerm.default_ttl_secs(),
                min_importance: 0.2,
                min_access_count: 1,
            },
        );
        tiers.insert(
            MemoryTier::LongTerm,
            TierConfig {
                enabled: true,
                capacity: MemoryTier::LongTerm.default_capacity(),
                ttl_secs: MemoryTier::LongTerm.default_ttl_secs(),
                min_importance: 0.5,
                min_access_count: 3,
            },
        );
        tiers.insert(
            MemoryTier::Archival,
            TierConfig {
                enabled: true,
                capacity: MemoryTier::Archival.default_capacity(),
                ttl_secs: MemoryTier::Archival.default_ttl_secs(),
                min_importance: 0.7,
                min_access_count: 5,
            },
        );

        Self {
            tiers,
            auto_promote: true,
            maintenance_interval_secs: 24 * 60 * 60, // Daily
        }
    }
}

/// A memory entry augmented with tier metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredMemory {
    /// The memory ID.
    pub id: String,
    /// Current tier.
    pub tier: MemoryTier,
    /// When the memory entered this tier.
    pub tier_entered_at: SystemTime,
    /// Number of times accessed.
    pub access_count: u32,
    /// Last access timestamp.
    pub last_accessed: Option<SystemTime>,
    /// Computed relevance score for ranking.
    pub relevance_score: f32,
}

/// Decision produced by the tier evaluator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TierAction {
    /// Promote to the given tier.
    Promote(MemoryTier),
    /// Demote to the given tier.
    Demote(MemoryTier),
    /// Keep in current tier.
    Keep,
    /// Evict (delete) the memory.
    Evict,
}

/// Evaluates whether a memory should move between tiers.
#[derive(Debug, Clone)]
pub struct TierEvaluator {
    config: TierSystemConfig,
}

impl TierEvaluator {
    /// Create a new evaluator with the given config.
    pub fn new(config: TierSystemConfig) -> Self {
        Self { config }
    }

    /// Evaluate a memory and decide its tier action.
    ///
    /// Factors: importance_score, access_count, age in current tier, config thresholds.
    pub fn evaluate(&self, memory: &super::Memory, tiered: &TieredMemory) -> TierAction {
        let Some(tier_config) = self.config.tiers.get(&tiered.tier) else {
            return TierAction::Keep;
        };

        if !tier_config.enabled {
            // Tier disabled — demote if possible, else evict
            return match tiered.tier.demote() {
                Some(lower) => TierAction::Demote(lower),
                None => TierAction::Evict,
            };
        }

        let now = SystemTime::now();
        let age_secs = now
            .duration_since(tiered.tier_entered_at)
            .unwrap_or_default()
            .as_secs();

        // Check TTL expiry
        if tier_config.ttl_secs > 0 && age_secs > tier_config.ttl_secs {
            // Expired — demote if possible
            return match tiered.tier.demote() {
                Some(lower) => TierAction::Demote(lower),
                None => TierAction::Evict,
            };
        }

        // Check promotion criteria
        if let Some(higher_tier) = tiered.tier.promote() {
            if let Some(higher_config) = self.config.tiers.get(&higher_tier) {
                if higher_config.enabled
                    && memory.importance_score >= higher_config.min_importance
                    && tiered.access_count >= higher_config.min_access_count
                {
                    return TierAction::Promote(higher_tier);
                }
            }
        }

        // Check demotion criteria (importance too low for current tier)
        if memory.importance_score < tier_config.min_importance {
            return match tiered.tier.demote() {
                Some(lower) => TierAction::Demote(lower),
                None => TierAction::Evict,
            };
        }

        TierAction::Keep
    }

    /// Determine if a new memory should enter at the given tier.
    pub fn entry_tier(&self, importance_score: f32, _access_count: u32) -> MemoryTier {
        if importance_score >= 0.7 {
            MemoryTier::LongTerm
        } else if importance_score >= 0.3 {
            MemoryTier::ShortTerm
        } else {
            MemoryTier::Working
        }
    }
}

/// In-memory tier index for fast tier-based queries.
#[derive(Debug, Default)]
pub struct TierIndex {
    /// Memory ID -> tiered metadata.
    entries: std::sync::RwLock<HashMap<String, TieredMemory>>,
}

impl TierIndex {
    /// Create a new empty tier index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a memory at the given tier.
    pub fn insert(&self, id: impl Into<String>, tier: MemoryTier) {
        let id = id.into();
        let entry = TieredMemory {
            id: id.clone(),
            tier,
            tier_entered_at: SystemTime::now(),
            access_count: 0,
            last_accessed: None,
            relevance_score: 0.5,
        };
        let mut guard = self.entries.write().unwrap();
        guard.insert(id, entry);
    }

    /// Record an access to a memory.
    pub fn record_access(&self, id: &str) {
        let mut guard = self.entries.write().unwrap();
        if let Some(entry) = guard.get_mut(id) {
            entry.access_count += 1;
            entry.last_accessed = Some(SystemTime::now());
        }
    }

    /// Update the tier of a memory.
    pub fn update_tier(&self, id: &str, new_tier: MemoryTier) {
        let mut guard = self.entries.write().unwrap();
        if let Some(entry) = guard.get_mut(id) {
            entry.tier = new_tier;
            entry.tier_entered_at = SystemTime::now();
        }
    }

    /// Get the tier of a memory.
    pub fn get_tier(&self, id: &str) -> Option<MemoryTier> {
        let guard = self.entries.read().unwrap();
        guard.get(id).map(|e| e.tier.clone())
    }

    /// Get tiered metadata for a memory.
    pub fn get(&self, id: &str) -> Option<TieredMemory> {
        let guard = self.entries.read().unwrap();
        guard.get(id).cloned()
    }

    /// Remove a memory from the index.
    pub fn remove(&self, id: &str) {
        let mut guard = self.entries.write().unwrap();
        guard.remove(id);
    }

    /// Count memories in each tier.
    pub fn counts_by_tier(&self) -> HashMap<MemoryTier, usize> {
        let guard = self.entries.read().unwrap();
        let mut counts = HashMap::new();
        for entry in guard.values() {
            *counts.entry(entry.tier.clone()).or_insert(0) += 1;
        }
        counts
    }

    /// List all memory IDs in a given tier.
    pub fn ids_in_tier(&self, tier: MemoryTier) -> Vec<String> {
        let guard = self.entries.read().unwrap();
        guard
            .values()
            .filter(|e| e.tier == tier)
            .map(|e| e.id.clone())
            .collect()
    }

    /// Total number of indexed memories.
    pub fn len(&self) -> usize {
        let guard = self.entries.read().unwrap();
        guard.len()
    }

    /// Check if the index is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_promotion() {
        assert_eq!(MemoryTier::Working.promote(), Some(MemoryTier::ShortTerm));
        assert_eq!(MemoryTier::ShortTerm.promote(), Some(MemoryTier::LongTerm));
        assert_eq!(MemoryTier::LongTerm.promote(), Some(MemoryTier::Archival));
        assert_eq!(MemoryTier::Archival.promote(), None);
    }

    #[test]
    fn test_tier_demotion() {
        assert_eq!(MemoryTier::Archival.demote(), Some(MemoryTier::LongTerm));
        assert_eq!(MemoryTier::LongTerm.demote(), Some(MemoryTier::ShortTerm));
        assert_eq!(MemoryTier::ShortTerm.demote(), Some(MemoryTier::Working));
        assert_eq!(MemoryTier::Working.demote(), None);
    }

    #[test]
    fn test_tier_evaluator_promote() {
        let config = TierSystemConfig::default();
        let evaluator = TierEvaluator::new(config);

        let memory =
            super::super::Memory::new("u1", "important fact", "fact").with_importance_score(0.8);

        let tiered = TieredMemory {
            id: "m1".to_string(),
            tier: MemoryTier::ShortTerm,
            tier_entered_at: SystemTime::now(),
            access_count: 5,
            last_accessed: None,
            relevance_score: 0.5,
        };

        let action = evaluator.evaluate(&memory, &tiered);
        assert_eq!(action, TierAction::Promote(MemoryTier::LongTerm));
    }

    #[test]
    fn test_tier_evaluator_demote_low_importance() {
        let config = TierSystemConfig::default();
        let evaluator = TierEvaluator::new(config);

        let memory = super::super::Memory::new("u1", "trivial", "fact").with_importance_score(0.1);

        let tiered = TieredMemory {
            id: "m1".to_string(),
            tier: MemoryTier::LongTerm,
            tier_entered_at: SystemTime::now(),
            access_count: 0,
            last_accessed: None,
            relevance_score: 0.5,
        };

        let action = evaluator.evaluate(&memory, &tiered);
        assert_eq!(action, TierAction::Demote(MemoryTier::ShortTerm));
    }

    #[test]
    fn test_tier_evaluator_evict_expired() {
        let mut config = TierSystemConfig::default();
        config.tiers.get_mut(&MemoryTier::Working).unwrap().ttl_secs = 1;
        let evaluator = TierEvaluator::new(config);

        let memory = super::super::Memory::new("u1", "old", "fact").with_importance_score(0.5);

        let tiered = TieredMemory {
            id: "m1".to_string(),
            tier: MemoryTier::Working,
            tier_entered_at: SystemTime::now() - std::time::Duration::from_secs(10),
            access_count: 0,
            last_accessed: None,
            relevance_score: 0.5,
        };

        let action = evaluator.evaluate(&memory, &tiered);
        // Working can't demote further, so it gets evicted
        assert_eq!(action, TierAction::Evict);
    }

    #[test]
    fn test_tier_index() {
        let index = TierIndex::new();
        index.insert("m1", MemoryTier::ShortTerm);
        index.insert("m2", MemoryTier::LongTerm);

        assert_eq!(index.get_tier("m1"), Some(MemoryTier::ShortTerm));
        assert_eq!(index.get_tier("m2"), Some(MemoryTier::LongTerm));

        index.record_access("m1");
        let entry = index.get("m1").unwrap();
        assert_eq!(entry.access_count, 1);
        assert!(entry.last_accessed.is_some());

        index.update_tier("m1", MemoryTier::LongTerm);
        assert_eq!(index.get_tier("m1"), Some(MemoryTier::LongTerm));

        let counts = index.counts_by_tier();
        assert_eq!(counts.get(&MemoryTier::LongTerm), Some(&2));
        assert_eq!(counts.get(&MemoryTier::ShortTerm), None);

        let long_term_ids = index.ids_in_tier(MemoryTier::LongTerm);
        assert_eq!(long_term_ids.len(), 2);
    }
}
