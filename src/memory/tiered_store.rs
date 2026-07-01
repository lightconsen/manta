//! Tiered Memory Store — routes operations across four tier-specific backends
//!
//! Architecture:
//! - Working    → InMemoryStore      (ephemeral, hot context)
//! - ShortTerm  → DatabaseStore      (SQLite, hours–days)
//! - LongTerm   → DatabaseStore      (SQLite, weeks–months)
//! - Archival   → CompressedJsonlStore (gzip JSONL, cold storage)
//!
//! The `TieredStore` implements `MemoryStore` by delegating to the appropriate
//! backend based on a `TierIndex`.  New memories are placed via
//! `TierEvaluator::entry_tier()`.  Search results are merged and re-sorted by
//! importance.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info, warn};

use super::{
    CompressedJsonlStore, DatabaseStore, EffectivenessConfig, InMemoryStore, Memory, MemoryId,
    MemoryQuery, MemoryStats, MemoryStore, MemoryTier, TierEvaluator, TierIndex, TierSystemConfig,
};

/// Aggregate store that routes each memory to its tier-specific backend.
#[derive(Debug)]
pub struct TieredStore {
    working: InMemoryStore,
    short_term: DatabaseStore,
    long_term: DatabaseStore,
    archival: CompressedJsonlStore,
    evaluator: Arc<TierEvaluator>,
    index: Arc<TierIndex>,
    last_stale_sweep: std::sync::atomic::AtomicU64,
}

impl Clone for TieredStore {
    fn clone(&self) -> Self {
        Self {
            working: self.working.clone(),
            short_term: self.short_term.clone(),
            long_term: self.long_term.clone(),
            archival: self.archival.clone(),
            evaluator: self.evaluator.clone(),
            index: self.index.clone(),
            last_stale_sweep: std::sync::atomic::AtomicU64::new(
                self.last_stale_sweep
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
        }
    }
}

/// Minimum interval between stale index sweeps in `cleanup_expired`.
const STALE_SWEEP_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(300);

impl TieredStore {
    /// Create a new tiered store with on-disk backends under `base_dir`.
    pub async fn new(base_dir: impl AsRef<Path>) -> crate::Result<Self> {
        let base = base_dir.as_ref();

        tokio::fs::create_dir_all(base)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to create tiered store directory: {:?}", base),
                details: e.to_string(),
            })?;

        let short_term =
            DatabaseStore::new(&format!("sqlite://{}/short_term.db", base.to_string_lossy()))
                .await?;

        let long_term =
            DatabaseStore::new(&format!("sqlite://{}/long_term.db", base.to_string_lossy()))
                .await?;

        Ok(Self {
            working: InMemoryStore::new(),
            short_term,
            long_term,
            archival: CompressedJsonlStore::new(base),
            evaluator: Arc::new(TierEvaluator::new(TierSystemConfig::default())),
            index: Arc::new(TierIndex::new()),
            last_stale_sweep: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Create a tiered store backed entirely by in-memory / temporary storage.
    /// Useful for tests.
    pub async fn new_in_memory() -> crate::Result<Self> {
        Ok(Self {
            working: InMemoryStore::new(),
            short_term: DatabaseStore::new_in_memory().await?,
            long_term: DatabaseStore::new_in_memory().await?,
            archival: CompressedJsonlStore::new(
                std::env::temp_dir()
                    .join(format!("syscity_archival_test_{}", uuid::Uuid::new_v4())),
            ),
            evaluator: Arc::new(TierEvaluator::new(TierSystemConfig::default())),
            index: Arc::new(TierIndex::new()),
            last_stale_sweep: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Build from pre-constructed backends (useful for tests or custom wiring).
    pub fn with_stores(
        working: InMemoryStore,
        short_term: DatabaseStore,
        long_term: DatabaseStore,
        archival: CompressedJsonlStore,
    ) -> Self {
        Self {
            working,
            short_term,
            long_term,
            archival,
            evaluator: Arc::new(TierEvaluator::new(TierSystemConfig::default())),
            index: Arc::new(TierIndex::new()),
            last_stale_sweep: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Clone the short-term store (for chat history delegation).
    pub fn short_term(&self) -> DatabaseStore {
        self.short_term.clone()
    }

    /// Clone the long-term store.
    pub fn long_term(&self) -> DatabaseStore {
        self.long_term.clone()
    }

    /// Replace the default evaluator.
    pub fn with_evaluator(mut self, evaluator: TierEvaluator) -> Self {
        self.evaluator = Arc::new(evaluator);
        self
    }

    /// Configure the evaluator with effectiveness thresholds.
    pub fn with_effectiveness_config(mut self, config: EffectivenessConfig) -> Self {
        let evaluator =
            TierEvaluator::new(TierSystemConfig::default()).with_effectiveness_config(config);
        self.evaluator = Arc::new(evaluator);
        self
    }

    /// Access the tier index (for testing and diagnostics).
    pub fn tier_index(&self) -> &Arc<TierIndex> {
        &self.index
    }

    /// Access the tier evaluator (for testing and diagnostics).
    pub fn evaluator(&self) -> &Arc<TierEvaluator> {
        &self.evaluator
    }

    /// Explicitly migrate a memory to a target tier.
    ///
    /// Used by the effectiveness feedback loop and dream scheduler
    /// to move memories between tiers based on importance/access changes.
    pub async fn migrate_memory(
        &self,
        memory: &Memory,
        target_tier: MemoryTier,
    ) -> crate::Result<()> {
        let id = &memory.id;

        // Find current tier
        let current_tier = if let Some(t) = self.index.get_tier(&id.0) {
            t
        } else {
            // Fallback scan
            let mut found = None;
            for tier in [
                MemoryTier::Working,
                MemoryTier::ShortTerm,
                MemoryTier::LongTerm,
                MemoryTier::Archival,
            ] {
                if self.backend_for(tier).get(id).await?.is_some() {
                    found = Some(tier);
                    break;
                }
            }
            match found {
                Some(t) => t,
                None => {
                    return Err(crate::error::SyscityError::NotFound {
                        resource: format!("Memory {}", id),
                    });
                }
            }
        };

        if current_tier == target_tier {
            return Ok(());
        }

        // Store in new backend FIRST, then delete from old backend.
        // This prevents data loss if the process crashes mid-migration:
        // a crash after store creates a harmless duplicate; a crash after
        // delete is equivalent to a successful migration.
        let memory_clone = memory.clone();
        self.backend_for(target_tier).store(memory_clone).await?;
        self.backend_for(current_tier).delete(id).await?;
        // Update index
        self.index.update_tier(&id.0, target_tier);

        info!("Memory {} explicitly migrated from {} to {}", id, current_tier, target_tier);
        Ok(())
    }

    /// Return the backend responsible for the given tier.
    fn backend_for(&self, tier: MemoryTier) -> &dyn MemoryStore {
        match tier {
            MemoryTier::Working => &self.working,
            MemoryTier::ShortTerm => &self.short_term,
            MemoryTier::LongTerm => &self.long_term,
            MemoryTier::Archival => &self.archival,
        }
    }

    /// Search all tiers with a bounded per-tier limit. Instead of pulling
    /// 10_000 rows from every tier (which is unbounded for large stores),
    /// each tier contributes at most `per_tier` rows.  Results are merged
    /// and re-sorted by importance, then the original limit/offset is applied.
    async fn search_all_tiers(&self, query: &MemoryQuery) -> crate::Result<Vec<Memory>> {
        if query.limit == 0 {
            return Ok(Vec::new());
        }

        // Pull enough per tier to cover the requested limit after merging
        // and re-sorting, but cap each tier to avoid unbounded queries on
        // large stores.  20 minimum ensures even tiny limits get diversity.
        let per_tier = (query.limit * 4).clamp(20, 2000);
        let mut unlimited = query.clone();
        unlimited.limit = per_tier;
        unlimited.offset = 0;

        let mut all = Vec::new();
        for tier in [
            MemoryTier::Working,
            MemoryTier::ShortTerm,
            MemoryTier::LongTerm,
            MemoryTier::Archival,
        ] {
            match self.backend_for(tier).search(unlimited.clone()).await {
                Ok(mut results) => all.append(&mut results),
                Err(e) => {
                    return Err(crate::error::SyscityError::Storage {
                        context: format!("Tier {:?} search failed", tier),
                        details: e.to_string(),
                    });
                }
            }
        }

        // Sort by importance descending (higher = more relevant)
        all.sort_by(|a, b| {
            b.importance_score
                .partial_cmp(&a.importance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let offset = query.offset.min(all.len());
        let limit = query.limit.min(all.len() - offset);
        Ok(all.into_iter().skip(offset).take(limit).collect())
    }
}

#[async_trait]
impl MemoryStore for TieredStore {
    async fn store(&self, memory: Memory) -> crate::Result<MemoryId> {
        let tier = self.evaluator.entry_tier(memory.importance_score, 0);
        let id = memory.id.clone();

        self.backend_for(tier).store(memory).await?;
        self.index.insert(&id.0, tier);

        debug!("Stored memory {} in {:?} tier", id, tier);
        Ok(id)
    }

    async fn get(&self, id: &MemoryId) -> crate::Result<Option<Memory>> {
        // Fast path: look up tier in index
        if let Some(tier) = self.index.get_tier(&id.0) {
            let result = self.backend_for(tier).get(id).await?;
            if result.is_some() {
                self.index.record_access(&id.0);
                return Ok(result);
            }
            // Stale index entry — fall through to scan
        }

        // Fallback: scan all backends
        for tier in [
            MemoryTier::Working,
            MemoryTier::ShortTerm,
            MemoryTier::LongTerm,
            MemoryTier::Archival,
        ] {
            let result = self.backend_for(tier).get(id).await?;
            if result.is_some() {
                self.index.insert(&id.0, tier);
                self.index.record_access(&id.0);
                return Ok(result);
            }
        }

        // Not found anywhere — clean up stale index entry
        self.index.remove(&id.0);
        Ok(None)
    }

    async fn update(&self, memory: Memory) -> crate::Result<()> {
        let id = memory.id.clone();

        // Fast path: known tier — check if migration is needed
        if let Some(current_tier) = self.index.get_tier(&id.0) {
            if let Some(tiered) = self.index.get(&id.0) {
                match self.evaluator.evaluate(&memory, &tiered, None) {
                    super::tier::TierAction::Keep => {
                        // Same tier, just update in place
                        return self.backend_for(current_tier).update(memory).await;
                    }
                    super::tier::TierAction::Promote(target)
                    | super::tier::TierAction::Demote(target) => {
                        // Migrate: store in new backend first, then delete from old
                        self.backend_for(target).store(memory).await?;
                        self.backend_for(current_tier).delete(&id).await?;
                        self.index.update_tier(&id.0, target);
                        info!(
                            "Memory {} migrated from {} to {} (importance={:.2}, access_count={})",
                            id, current_tier, target, tiered.relevance_score, tiered.access_count
                        );
                        return Ok(());
                    }
                    super::tier::TierAction::Evict => {
                        self.backend_for(current_tier).delete(&id).await?;
                        self.index.remove(&id.0);
                        info!("Memory {} evicted from {} via update", id, current_tier);
                        return Err(crate::error::SyscityError::NotFound {
                            resource: format!("Memory {} was evicted during update", id),
                        });
                    }
                }
            }
            // No tiered metadata found in index — update in place and trust the index
            return self.backend_for(current_tier).update(memory).await;
        }

        // Fallback: scan all backends
        for tier in [
            MemoryTier::Working,
            MemoryTier::ShortTerm,
            MemoryTier::LongTerm,
            MemoryTier::Archival,
        ] {
            if let Some(existing) = self.backend_for(tier).get(&id).await? {
                // Check if the memory should migrate based on its new state
                let existing_access_count =
                    self.index.get(&id.0).map(|t| t.access_count).unwrap_or(1); // Default to 1 so promotion isn't blocked
                let tiered = super::tier::TieredMemory {
                    id: id.0.clone(),
                    tier,
                    tier_entered_at: std::time::SystemTime::now(),
                    access_count: existing_access_count,
                    last_accessed: None,
                    relevance_score: existing.importance_score,
                };
                match self.evaluator.evaluate(&memory, &tiered, None) {
                    super::tier::TierAction::Keep => {
                        self.backend_for(tier).update(memory).await?;
                        self.index.insert(&id.0, tier);
                    }
                    super::tier::TierAction::Promote(target)
                    | super::tier::TierAction::Demote(target) => {
                        self.backend_for(target).store(memory).await?;
                        self.backend_for(tier).delete(&id).await?;
                        self.index.insert(&id.0, target);
                        info!(
                            "Memory {} migrated from {} to {} during fallback scan",
                            id, tier, target
                        );
                    }
                    super::tier::TierAction::Evict => {
                        self.backend_for(tier).delete(&id).await?;
                        // Do not re-insert into index
                        info!("Memory {} evicted during fallback scan", id);
                    }
                }
                return Ok(());
            }
        }

        Err(crate::error::SyscityError::NotFound {
            resource: format!("Memory {}", id),
        })
    }

    async fn update_importance_score(
        &self,
        id: &MemoryId,
        new_score: f32,
    ) -> crate::Result<Option<Memory>> {
        // Fast path: known tier — update in place without triggering migration.
        if let Some(tier) = self.index.get_tier(&id.0) {
            return self
                .backend_for(tier)
                .update_importance_score(id, new_score)
                .await;
        }

        // Fallback: scan all backends and update the first match.
        for tier in [
            MemoryTier::Working,
            MemoryTier::ShortTerm,
            MemoryTier::LongTerm,
            MemoryTier::Archival,
        ] {
            if let Some(updated) = self
                .backend_for(tier)
                .update_importance_score(id, new_score)
                .await?
            {
                self.index.insert(&id.0, tier);
                return Ok(Some(updated));
            }
        }

        Ok(None)
    }

    async fn delete(&self, id: &MemoryId) -> crate::Result<bool> {
        if let Some(tier) = self.index.get_tier(&id.0) {
            let deleted = self.backend_for(tier).delete(id).await?;
            if deleted {
                self.index.remove(&id.0);
            }
            return Ok(deleted);
        }

        // Fallback scan
        for tier in [
            MemoryTier::Working,
            MemoryTier::ShortTerm,
            MemoryTier::LongTerm,
            MemoryTier::Archival,
        ] {
            if self.backend_for(tier).delete(id).await? {
                self.index.remove(&id.0);
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn search(&self, query: MemoryQuery) -> crate::Result<Vec<Memory>> {
        self.search_all_tiers(&query).await
    }

    async fn cleanup_expired(&self) -> crate::Result<usize> {
        let mut total = 0;
        for tier in [
            MemoryTier::Working,
            MemoryTier::ShortTerm,
            MemoryTier::LongTerm,
            MemoryTier::Archival,
        ] {
            match self.backend_for(tier).cleanup_expired().await {
                Ok(removed) => total += removed,
                Err(e) => {
                    warn!("Tier {:?} cleanup failed: {}", tier, e);
                }
            }
        }
        // Throttle stale index sweep to avoid O(n) per-tier SQL queries on every call.
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let last = self
            .last_stale_sweep
            .load(std::sync::atomic::Ordering::Relaxed);
        if now_secs.saturating_sub(last) < STALE_SWEEP_COOLDOWN.as_secs() {
            return Ok(total);
        }

        // Remove stale index entries for indexed backends only.
        // Skip Archival tier because CompressedJsonlStore.get() decompresses
        // ALL shards — O(n) per call — making a per-memory loop O(n²).
        let stale_tiers = [
            MemoryTier::Working,
            MemoryTier::ShortTerm,
            MemoryTier::LongTerm,
        ];
        let stale_ids: Vec<String> = {
            let mut stale = Vec::new();
            for tier in stale_tiers {
                for mem_id in self.index.ids_in_tier(tier) {
                    let mid = MemoryId::new(&mem_id);
                    if self.backend_for(tier).get(&mid).await?.is_none() {
                        stale.push(mem_id);
                    }
                }
            }
            stale
        };
        self.last_stale_sweep
            .store(now_secs, std::sync::atomic::Ordering::Relaxed);
        for id in &stale_ids {
            self.index.remove(id);
        }
        if !stale_ids.is_empty() {
            debug!(
                "Cleaned {} stale index entries from cleanup_expired (skipped Archival tier)",
                stale_ids.len()
            );
        }

        info!("Cleaned up {} expired memories across all tiers", total);
        Ok(total)
    }

    async fn stats(&self) -> crate::Result<MemoryStats> {
        let mut total_count = 0;
        let mut count_by_type: HashMap<String, usize> = HashMap::new();
        let mut expired_count = 0;

        for tier in [
            MemoryTier::Working,
            MemoryTier::ShortTerm,
            MemoryTier::LongTerm,
            MemoryTier::Archival,
        ] {
            match self.backend_for(tier).stats().await {
                Ok(stats) => {
                    total_count += stats.total_count;
                    for (k, v) in stats.count_by_type {
                        *count_by_type.entry(k).or_insert(0) += v;
                    }
                    expired_count += stats.expired_count;
                }
                Err(e) => {
                    warn!("Tier {:?} stats failed: {}", tier, e);
                }
            }
        }

        Ok(MemoryStats {
            total_count,
            count_by_type,
            expired_count,
        })
    }

    async fn close(&self) -> crate::Result<()> {
        self.working.close().await?;
        self.short_term.close().await?;
        self.long_term.close().await?;
        self.archival.close().await?;
        info!("TieredStore closed all backends");
        Ok(())
    }

    fn as_tiered_store(&self) -> Option<&TieredStore> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tiered_store_routes_by_importance() {
        let store = TieredStore::new_in_memory().await.unwrap();

        // Low importance → Working
        let low = Memory::new("u1", "Low priority", "fact").with_importance_score(0.1);
        let id_low = store.store(low).await.unwrap();

        // Medium importance → ShortTerm
        let med = Memory::new("u1", "Medium priority", "fact").with_importance_score(0.4);
        let id_med = store.store(med).await.unwrap();

        // High importance → LongTerm
        let high = Memory::new("u1", "High priority", "fact").with_importance_score(0.8);
        let id_high = store.store(high).await.unwrap();

        // Verify retrieval via tiered store
        assert_eq!(store.get(&id_low).await.unwrap().unwrap().content, "Low priority");
        assert_eq!(store.get(&id_med).await.unwrap().unwrap().content, "Medium priority");
        assert_eq!(store.get(&id_high).await.unwrap().unwrap().content, "High priority");

        // Verify tier index tracking
        assert_eq!(store.index.get_tier(&id_low.0), Some(MemoryTier::Working));
        assert_eq!(store.index.get_tier(&id_med.0), Some(MemoryTier::ShortTerm));
        assert_eq!(store.index.get_tier(&id_high.0), Some(MemoryTier::LongTerm));
    }

    #[tokio::test]
    async fn test_tiered_search_merges_results() {
        let store = TieredStore::new_in_memory().await.unwrap();

        store
            .store(Memory::new("u1", "Working memory", "fact").with_importance_score(0.1))
            .await
            .unwrap();
        store
            .store(Memory::new("u1", "Short term note", "note").with_importance_score(0.4))
            .await
            .unwrap();
        store
            .store(Memory::new("u1", "Long term fact", "fact").with_importance_score(0.8))
            .await
            .unwrap();

        let results = store
            .search(MemoryQuery::new().for_user("u1").limit(10))
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        // Should be sorted by importance descending
        assert!(results[0].importance_score >= results[1].importance_score);
        assert!(results[1].importance_score >= results[2].importance_score);
    }

    #[tokio::test]
    async fn test_tiered_delete_and_stats() {
        let store = TieredStore::new_in_memory().await.unwrap();

        let mem = Memory::new("u1", "Delete me", "fact").with_importance_score(0.5);
        let id = store.store(mem).await.unwrap();

        let stats_before = store.stats().await.unwrap();
        assert_eq!(stats_before.total_count, 1);

        let deleted = store.delete(&id).await.unwrap();
        assert!(deleted);

        let stats_after = store.stats().await.unwrap();
        assert_eq!(stats_after.total_count, 0);

        assert!(store.get(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_tiered_update_across_tiers() {
        let store = TieredStore::new_in_memory().await.unwrap();

        let mem = Memory::new("u1", "Original", "fact").with_importance_score(0.5);
        let id = store.store(mem.clone()).await.unwrap();

        let mut updated = mem;
        updated.content = "Updated".to_string();
        updated.id = id.clone();

        store.update(updated).await.unwrap();

        let fetched = store.get(&id).await.unwrap().unwrap();
        assert_eq!(fetched.content, "Updated");
    }

    #[tokio::test]
    async fn test_tiered_update_triggers_promotion() {
        let store = TieredStore::new_in_memory().await.unwrap();

        // Start with low importance → Working tier
        let mem = Memory::new("u1", "Promote me", "fact").with_importance_score(0.1);
        let id = store.store(mem.clone()).await.unwrap();

        // Manually bump access count so promotion criteria are met
        store.index.record_access(&id.0);
        store.index.record_access(&id.0);
        store.index.record_access(&id.0);

        assert_eq!(store.index.get_tier(&id.0), Some(MemoryTier::Working));

        // Increase importance to trigger promotion
        let mut updated = mem.clone();
        updated.id = id.clone();
        updated.importance_score = 0.8;

        store.update(updated.clone()).await.unwrap();

        // TierEvaluator promotes one tier at a time: Working → ShortTerm
        assert_eq!(
            store.index.get_tier(&id.0),
            Some(MemoryTier::ShortTerm),
            "Memory should have been promoted to ShortTerm on first update"
        );

        // Second update promotes ShortTerm → LongTerm
        store.update(updated.clone()).await.unwrap();

        assert_eq!(
            store.index.get_tier(&id.0),
            Some(MemoryTier::LongTerm),
            "Memory should have been promoted to LongTerm on second update"
        );

        // Should still be retrievable
        let fetched = store.get(&id).await.unwrap().unwrap();
        assert_eq!(fetched.content, "Promote me");
        assert_eq!(fetched.importance_score, 0.8);
    }

    #[tokio::test]
    async fn test_tiered_update_triggers_demotion() {
        let store = TieredStore::new_in_memory().await.unwrap();

        // Start with high importance → LongTerm tier
        let mem = Memory::new("u1", "Demote me", "fact").with_importance_score(0.8);
        let id = store.store(mem.clone()).await.unwrap();

        assert_eq!(store.index.get_tier(&id.0), Some(MemoryTier::LongTerm));

        // Decrease importance below LongTerm threshold (0.5)
        let mut updated = mem.clone();
        updated.id = id.clone();
        updated.importance_score = 0.3;

        store.update(updated).await.unwrap();

        // Should have demoted to ShortTerm (importance < 0.5 but >= 0.2)
        assert_eq!(
            store.index.get_tier(&id.0),
            Some(MemoryTier::ShortTerm),
            "Memory should have been demoted to ShortTerm"
        );

        let fetched = store.get(&id).await.unwrap().unwrap();
        assert_eq!(fetched.importance_score, 0.3);
    }

    #[tokio::test]
    async fn test_tiered_update_no_change_when_tier_kept() {
        let store = TieredStore::new_in_memory().await.unwrap();

        // Medium importance → ShortTerm
        let mem = Memory::new("u1", "Stay put", "fact").with_importance_score(0.4);
        let id = store.store(mem.clone()).await.unwrap();

        assert_eq!(store.index.get_tier(&id.0), Some(MemoryTier::ShortTerm));

        // Small change still within ShortTerm range
        let mut updated = mem.clone();
        updated.id = id.clone();
        updated.importance_score = 0.45;

        store.update(updated).await.unwrap();

        // Should stay in ShortTerm
        assert_eq!(store.index.get_tier(&id.0), Some(MemoryTier::ShortTerm));
    }

    #[tokio::test]
    async fn test_migrate_memory_explicit() {
        let store = TieredStore::new_in_memory().await.unwrap();

        let mem = Memory::new("u1", "Explicit move", "fact").with_importance_score(0.5);
        let id = store.store(mem.clone()).await.unwrap();

        assert_eq!(store.index.get_tier(&id.0), Some(MemoryTier::ShortTerm));

        // Explicitly migrate to Archival
        store
            .migrate_memory(&mem, MemoryTier::Archival)
            .await
            .unwrap();

        assert_eq!(store.index.get_tier(&id.0), Some(MemoryTier::Archival));

        let fetched = store.get(&id).await.unwrap().unwrap();
        assert_eq!(fetched.content, "Explicit move");
    }
}
