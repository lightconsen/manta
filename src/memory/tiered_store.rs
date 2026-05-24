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

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::{
    CompressedJsonlStore, DatabaseStore, InMemoryStore, Memory, MemoryId, MemoryQuery, MemoryStats,
    MemoryStore, MemoryTier, TierEvaluator, TierIndex, TierSystemConfig,
};

/// Aggregate store that routes each memory to its tier-specific backend.
#[derive(Clone, Debug)]
pub struct TieredStore {
    working: InMemoryStore,
    short_term: DatabaseStore,
    long_term: DatabaseStore,
    archival: CompressedJsonlStore,
    evaluator: Arc<TierEvaluator>,
    index: Arc<TierIndex>,
}

impl TieredStore {
    /// Create a new tiered store with on-disk backends under `base_dir`.
    pub async fn new(base_dir: impl AsRef<Path>) -> crate::Result<Self> {
        let base = base_dir.as_ref();

        tokio::fs::create_dir_all(base)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
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
        })
    }

    /// Create a tiered store backed entirely by in-memory / temporary storage.
    /// Useful for tests.
    pub async fn new_in_memory() -> crate::Result<Self> {
        Ok(Self {
            working: InMemoryStore::new(),
            short_term: DatabaseStore::new_in_memory().await?,
            long_term: DatabaseStore::new_in_memory().await?,
            archival: CompressedJsonlStore::new(std::env::temp_dir().join("manta_archival_test")),
            evaluator: Arc::new(TierEvaluator::new(TierSystemConfig::default())),
            index: Arc::new(TierIndex::new()),
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

    /// Return the backend responsible for the given tier.
    fn backend_for(&self, tier: MemoryTier) -> &dyn MemoryStore {
        match tier {
            MemoryTier::Working => &self.working,
            MemoryTier::ShortTerm => &self.short_term,
            MemoryTier::LongTerm => &self.long_term,
            MemoryTier::Archival => &self.archival,
        }
    }

    /// Search all tiers with an unlimited query, merge, sort, and apply
    /// the original limit / offset.
    async fn search_all_tiers(&self, query: &MemoryQuery) -> crate::Result<Vec<Memory>> {
        let mut unlimited = query.clone();
        unlimited.limit = 10_000;
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
                    warn!("Tier {:?} search failed: {}", tier, e);
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

        if let Some(tier) = self.index.get_tier(&id.0) {
            self.backend_for(tier).update(memory).await?;
            return Ok(());
        }

        // Fallback: scan all backends
        for tier in [
            MemoryTier::Working,
            MemoryTier::ShortTerm,
            MemoryTier::LongTerm,
            MemoryTier::Archival,
        ] {
            if self.backend_for(tier).get(&id).await?.is_some() {
                self.backend_for(tier).update(memory).await?;
                self.index.insert(&id.0, tier);
                return Ok(());
            }
        }

        Err(crate::error::MantaError::NotFound {
            resource: format!("Memory {}", id),
        })
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
        // Stale index entries are lazily cleaned on next access.
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
}
