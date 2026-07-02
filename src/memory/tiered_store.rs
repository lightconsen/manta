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

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info, warn};

use super::{
    CompressedJsonlStore, DatabaseStore, EffectivenessConfig, InMemoryStore, Memory,
    MemoryEntryType, MemoryId, MemoryQuery, MemoryStats, MemoryStore, MemoryTier, TierEvaluator,
    TierIndex, TierSystemConfig, TIER_INDEX_FILE_NAME,
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
    index_path: std::path::PathBuf,
    last_stale_sweep: std::sync::atomic::AtomicU64,
    expected_embedding_dim: Option<usize>,
    /// Fixed-size async lock pool used to serialize mutating operations on the
    /// same memory id across tiers. This eliminates duplicate-data races
    /// between concurrent migrate/update/delete calls without unbounded lock
    /// growth.
    memory_locks: Arc<[tokio::sync::Mutex<()>; 256]>,
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
            index_path: self.index_path.clone(),
            last_stale_sweep: std::sync::atomic::AtomicU64::new(
                self.last_stale_sweep
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            expected_embedding_dim: self.expected_embedding_dim,
            memory_locks: Arc::clone(&self.memory_locks),
        }
    }
}

/// Minimum interval between stale index sweeps in `cleanup_expired`.
const STALE_SWEEP_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(300);

/// Number of slots in the per-memory mutation lock pool.
const MEMORY_LOCK_POOL_SIZE: usize = 256;

/// Create a fixed-size pool of tokio mutexes used to serialize mutating
/// operations on the same memory id.
fn new_memory_lock_pool() -> Arc<[tokio::sync::Mutex<()>; MEMORY_LOCK_POOL_SIZE]> {
    Arc::new(std::array::from_fn(|_| tokio::sync::Mutex::new(())))
}

/// Map a memory id to a lock-pool index.
fn memory_lock_index(id: &str) -> usize {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    (hasher.finish() as usize) % MEMORY_LOCK_POOL_SIZE
}

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

        let default_dim = 384;

        let short_term =
            DatabaseStore::new(&format!("sqlite://{}/short_term.db", base.to_string_lossy()))
                .await?
                .with_embedding_dimension(default_dim);

        let long_term =
            DatabaseStore::new(&format!("sqlite://{}/long_term.db", base.to_string_lossy()))
                .await?
                .with_embedding_dimension(default_dim);

        let index_path = base.join(TIER_INDEX_FILE_NAME);
        let index = match TierIndex::load(&index_path) {
            Ok(idx) => Arc::new(idx),
            Err(e) => {
                warn!(
                    "Failed to load tier index from {:?}: {}. Starting with empty index.",
                    index_path, e
                );
                Arc::new(TierIndex::new())
            }
        };

        Ok(Self {
            working: InMemoryStore::new(),
            short_term,
            long_term,
            archival: CompressedJsonlStore::new(base),
            evaluator: Arc::new(TierEvaluator::new(TierSystemConfig::default())),
            index,
            index_path,
            last_stale_sweep: std::sync::atomic::AtomicU64::new(0),
            expected_embedding_dim: Some(default_dim),
            memory_locks: new_memory_lock_pool(),
        })
    }

    /// Set expected embedding dimension for validation
    pub fn with_embedding_dimension(mut self, dimension: usize) -> Self {
        self.expected_embedding_dim = Some(dimension);
        self.short_term = self.short_term.with_embedding_dimension(dimension);
        self.long_term = self.long_term.with_embedding_dimension(dimension);
        self
    }

    /// Create a tiered store backed entirely by in-memory / temporary storage.
    /// Useful for tests.
    pub async fn new_in_memory() -> crate::Result<Self> {
        let temp_dir =
            std::env::temp_dir().join(format!("syscity_archival_test_{}", uuid::Uuid::new_v4()));
        let default_dim = 384;
        Ok(Self {
            working: InMemoryStore::new(),
            short_term: DatabaseStore::new_in_memory()
                .await?
                .with_embedding_dimension(default_dim),
            long_term: DatabaseStore::new_in_memory()
                .await?
                .with_embedding_dimension(default_dim),
            archival: CompressedJsonlStore::new(&temp_dir),
            evaluator: Arc::new(TierEvaluator::new(TierSystemConfig::default())),
            index: Arc::new(TierIndex::new()),
            index_path: temp_dir.join(TIER_INDEX_FILE_NAME),
            last_stale_sweep: std::sync::atomic::AtomicU64::new(0),
            expected_embedding_dim: Some(default_dim),
            memory_locks: new_memory_lock_pool(),
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
            index_path: std::env::temp_dir().join(TIER_INDEX_FILE_NAME),
            last_stale_sweep: std::sync::atomic::AtomicU64::new(0),
            expected_embedding_dim: None,
            memory_locks: new_memory_lock_pool(),
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

    /// Persist the tier index to disk. Errors are logged but not propagated
    /// so that index persistence never breaks the primary storage operation.
    async fn persist_index(&self) {
        if let Err(e) = self.index.save(&self.index_path).await {
            warn!("Failed to persist tier index to {:?}: {}", self.index_path, e);
        }
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
    ///
    /// The operation is idempotent: if the memory already exists in the target
    /// tier (e.g., after a crash left a duplicate), the source copy is removed
    /// and the index is reconciled.
    pub async fn migrate_memory(
        &self,
        memory: &Memory,
        target_tier: MemoryTier,
    ) -> crate::Result<()> {
        let id = &memory.id;
        let _guard = self.memory_locks[memory_lock_index(&id.0)].lock().await;
        self.migrate_memory_unlocked(memory, target_tier).await
    }

    /// Migrate a memory to a target tier without acquiring the per-memory lock.
    ///
    /// # Safety
    ///
    /// The caller must already hold the per-memory lock for `memory.id` so the
    /// migration is atomic with respect to other mutating operations on the
    /// same memory id.
    pub(crate) async fn migrate_memory_unlocked(
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

        // Idempotency: if a previous crash already copied the memory to the
        // target tier, just remove the source copy and update the index.
        if self.backend_for(target_tier).get(id).await?.is_some() {
            self.backend_for(current_tier).delete(id).await?;
            self.index.update_tier(&id.0, target_tier);
            self.persist_index().await;
            info!(
                "Memory {} migrated from {} to {} (idempotent: target already held the memory)",
                id, current_tier, target_tier
            );
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
        self.persist_index().await;

        info!("Memory {} explicitly migrated from {} to {}", id, current_tier, target_tier);
        Ok(())
    }

    /// Acquire the per-memory mutation lock for `id`.
    ///
    /// Callers (e.g. `MemoryManager`) can hold this guard across a sequence of
    /// get/evaluate/update/migrate operations so they appear atomic with
    /// respect to other mutating operations on the same memory id.
    pub(crate) async fn lock_memory(&self, id: &str) -> tokio::sync::MutexGuard<'_, ()> {
        self.memory_locks[memory_lock_index(id)].lock().await
    }

    /// Update a memory's importance score without acquiring the per-memory
    /// lock.
    ///
    /// # Safety
    ///
    /// The caller must already hold the per-memory lock for `id` (e.g. via
    /// [`Self::lock_memory`]) so the read/update pair is atomic with respect to
    /// other mutating operations on the same memory id.
    pub(crate) async fn update_importance_score_unlocked(
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

        // Query all tiers concurrently. Each backend is independent, so this
        // reduces latency from sum(tier_latency) to max(tier_latency).
        let (working, short_term, long_term, archival) = tokio::join!(
            self.working.search(unlimited.clone()),
            self.short_term.search(unlimited.clone()),
            self.long_term.search(unlimited.clone()),
            self.archival.search(unlimited.clone()),
        );

        let mut all = Vec::new();
        let mut seen = HashSet::new();
        for (tier, result) in [
            (MemoryTier::Working, working),
            (MemoryTier::ShortTerm, short_term),
            (MemoryTier::LongTerm, long_term),
            (MemoryTier::Archival, archival),
        ] {
            match result {
                Ok(results) => {
                    for mem in results {
                        if seen.insert(mem.id.0.clone()) {
                            all.push(mem);
                        }
                    }
                }
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

        // Serialize mutations for the same memory id, even on initial store,
        // so callers that supply deterministic or externally chosen ids cannot
        // create duplicate tier entries.
        let _guard = self.memory_locks[memory_lock_index(&id.0)].lock().await;

        self.backend_for(tier).store(memory).await?;
        self.index.insert(&id.0, tier);
        self.persist_index().await;

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
        let _guard = self.memory_locks[memory_lock_index(&id.0)].lock().await;

        // Fast path: known tier — check if migration is needed
        if let Some(current_tier) = self.index.get_tier(&id.0) {
            if let Some(tiered) = self.index.get(&id.0) {
                match self.evaluator.evaluate(&memory, &tiered, None) {
                    super::tier::TierAction::Keep => {
                        // Same tier, just update in place
                        self.backend_for(current_tier).update(memory).await?;
                        self.persist_index().await;
                        return Ok(());
                    }
                    super::tier::TierAction::Promote(target)
                    | super::tier::TierAction::Demote(target) => {
                        // Migrate: store in new backend first, then delete from old
                        self.backend_for(target).store(memory).await?;
                        self.backend_for(current_tier).delete(&id).await?;
                        self.index.update_tier(&id.0, target);
                        self.persist_index().await;
                        info!(
                            "Memory {} migrated from {} to {} (importance={:.2}, access_count={})",
                            id, current_tier, target, tiered.relevance_score, tiered.access_count
                        );
                        return Ok(());
                    }
                    super::tier::TierAction::Evict => {
                        self.backend_for(current_tier).delete(&id).await?;
                        self.index.remove(&id.0);
                        self.persist_index().await;
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
                        self.persist_index().await;
                    }
                    super::tier::TierAction::Promote(target)
                    | super::tier::TierAction::Demote(target) => {
                        self.backend_for(target).store(memory).await?;
                        self.backend_for(tier).delete(&id).await?;
                        self.index.insert(&id.0, target);
                        self.persist_index().await;
                        info!(
                            "Memory {} migrated from {} to {} during fallback scan",
                            id, tier, target
                        );
                    }
                    super::tier::TierAction::Evict => {
                        self.backend_for(tier).delete(&id).await?;
                        // Do not re-insert into index
                        self.persist_index().await;
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
        let _guard = self.memory_locks[memory_lock_index(&id.0)].lock().await;
        self.update_importance_score_unlocked(id, new_score).await
    }

    async fn delete(&self, id: &MemoryId) -> crate::Result<bool> {
        let _guard = self.memory_locks[memory_lock_index(&id.0)].lock().await;

        if let Some(tier) = self.index.get_tier(&id.0) {
            let deleted = self.backend_for(tier).delete(id).await?;
            if deleted {
                self.index.remove(&id.0);
                self.persist_index().await;
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
                self.persist_index().await;
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
        let mut stale: Vec<String> = Vec::new();
        for tier in stale_tiers {
            for mem_id in self.index.ids_in_tier(tier) {
                let mid = MemoryId::new(&mem_id);
                if self.backend_for(tier).get(&mid).await?.is_none() {
                    stale.push(mem_id);
                }
            }
        }

        // Archival stale-index sweep: bulk-load all ids once to avoid O(n²)
        // per-get decompression. The sweep is throttled, so the occasional
        // full scan is acceptable for cold storage.
        let archival_ids: HashSet<String> = self
            .archival
            .search(MemoryQuery::new().limit(usize::MAX))
            .await?
            .into_iter()
            .map(|m| m.id.0)
            .collect();
        for mem_id in self.index.ids_in_tier(MemoryTier::Archival) {
            if !archival_ids.contains(&mem_id) {
                stale.push(mem_id);
            }
        }

        self.last_stale_sweep
            .store(now_secs, std::sync::atomic::Ordering::Relaxed);
        for id in &stale {
            self.index.remove(id);
        }
        if !stale.is_empty() {
            debug!("Cleaned {} stale index entries from cleanup_expired", stale.len());
        }

        info!("Cleaned up {} expired memories across all tiers", total);
        self.persist_index().await;
        Ok(total)
    }

    async fn stats(&self) -> crate::Result<MemoryStats> {
        let mut total_count = 0;
        let mut count_by_type: HashMap<MemoryEntryType, usize> = HashMap::new();
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
        self.persist_index().await;
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

    #[tokio::test]
    async fn test_migrate_memory_idempotent() {
        let store = TieredStore::new_in_memory().await.unwrap();

        let mem = Memory::new("u1", "Crash survivor", "fact").with_importance_score(0.5);
        let id = store.store(mem.clone()).await.unwrap();
        assert_eq!(store.index.get_tier(&id.0), Some(MemoryTier::ShortTerm));

        // Simulate a previous crash that already copied the memory to the
        // target tier while the index still points at the source tier.
        store.long_term.store(mem.clone()).await.unwrap();

        // The migration must detect the duplicate and reconcile the index.
        store
            .migrate_memory(&mem, MemoryTier::LongTerm)
            .await
            .unwrap();

        assert_eq!(store.index.get_tier(&id.0), Some(MemoryTier::LongTerm));

        // The memory must exist in exactly one tier.
        let mut count = 0usize;
        for tier in [
            MemoryTier::Working,
            MemoryTier::ShortTerm,
            MemoryTier::LongTerm,
            MemoryTier::Archival,
        ] {
            let backend: &dyn MemoryStore = match tier {
                MemoryTier::Working => &store.working,
                MemoryTier::ShortTerm => &store.short_term,
                MemoryTier::LongTerm => &store.long_term,
                MemoryTier::Archival => &store.archival,
            };
            if backend.get(&id).await.unwrap().is_some() {
                count += 1;
            }
        }
        assert_eq!(
            count, 1,
            "memory should exist in exactly one backend after idempotent migration"
        );

        let fetched = store.get(&id).await.unwrap().unwrap();
        assert_eq!(fetched.content, "Crash survivor");
    }

    #[tokio::test]
    async fn test_concurrent_migrate_no_duplicate() {
        let store = TieredStore::new_in_memory().await.unwrap();

        let mem = Memory::new("u1", "Race me", "fact").with_importance_score(0.5);
        let id = store.store(mem.clone()).await.unwrap();

        let store_a = store.clone();
        let store_b = store.clone();
        let mem_a = mem.clone();
        let mem_b = mem.clone();
        let id_a = id.clone();
        let _id_b = id.clone();

        let (res_a, res_b) = tokio::join!(
            async move { store_a.migrate_memory(&mem_a, MemoryTier::LongTerm).await },
            async move { store_b.migrate_memory(&mem_b, MemoryTier::Archival).await },
        );

        // At least one migration should succeed; the lock serializes them.
        assert!(res_a.is_ok() || res_b.is_ok());

        // The memory must exist in exactly one tier.
        let mut count = 0usize;
        for tier in [
            MemoryTier::Working,
            MemoryTier::ShortTerm,
            MemoryTier::LongTerm,
            MemoryTier::Archival,
        ] {
            let backend: &dyn MemoryStore = match tier {
                MemoryTier::Working => &store.working,
                MemoryTier::ShortTerm => &store.short_term,
                MemoryTier::LongTerm => &store.long_term,
                MemoryTier::Archival => &store.archival,
            };
            if backend.get(&id_a).await.unwrap().is_some() {
                count += 1;
            }
        }

        assert_eq!(
            count, 1,
            "memory should exist in exactly one backend after concurrent migrations"
        );

        // Search must return exactly one copy.
        let results = store
            .search(MemoryQuery::new().for_user("u1").limit(10))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_search_all_tiers_dedup() {
        let store = TieredStore::new_in_memory().await.unwrap();

        let mem = Memory::new("u1", "Duplicate", "fact").with_importance_score(0.5);
        let id = mem.id.clone();

        // Store the same memory in two backends directly to simulate a
        // crash that left a duplicate behind.
        store.working.store(mem.clone()).await.unwrap();
        store.short_term.store(mem.clone()).await.unwrap();
        store.index.insert(&id.0, MemoryTier::Working);

        let results = store
            .search(MemoryQuery::new().for_user("u1").limit(10))
            .await
            .unwrap();

        assert_eq!(results.len(), 1, "duplicate memories should be deduplicated");
    }

    #[tokio::test]
    async fn test_cleanup_expired_removes_archival_stale_index() {
        let store = TieredStore::new_in_memory().await.unwrap();

        let mem = Memory::new("u1", "Stale archival", "fact").with_importance_score(0.1);
        let id = store.store(mem.clone()).await.unwrap();

        // Migrate to Archival, then delete from Archival behind the index's back.
        store
            .migrate_memory(&mem, MemoryTier::Archival)
            .await
            .unwrap();
        assert_eq!(store.index.get_tier(&id.0), Some(MemoryTier::Archival));
        store.archival.delete(&id).await.unwrap();

        // cleanup_expired should remove the stale Archival index entry.
        store.cleanup_expired().await.unwrap();
        assert!(store.index.get_tier(&id.0).is_none());
    }

    #[tokio::test]
    async fn test_store_acquires_memory_lock() {
        let store = TieredStore::new_in_memory().await.unwrap();

        // Use a deterministic id so two concurrent stores target the same key.
        let id = MemoryId::new("deterministic-shared-id");
        let mem_a = Memory {
            id: id.clone(),
            user_id: "u1".to_string(),
            conversation_id: None,
            content: "A".to_string(),
            memory_type: MemoryEntryType::Fact,
            embedding: None,
            created_at: std::time::SystemTime::now(),
            last_accessed: std::time::SystemTime::now(),
            access_count: 0,
            expires_at: None,
            metadata: None,
            importance_score: 0.5,
            source: "agent".to_string(),
        };
        let mem_b = Memory {
            id: id.clone(),
            user_id: "u1".to_string(),
            conversation_id: None,
            content: "B".to_string(),
            memory_type: MemoryEntryType::Fact,
            embedding: None,
            created_at: std::time::SystemTime::now(),
            last_accessed: std::time::SystemTime::now(),
            access_count: 0,
            expires_at: None,
            metadata: None,
            importance_score: 0.5,
            source: "agent".to_string(),
        };

        let store_a = store.clone();
        let store_b = store.clone();

        let (res_a, res_b) = tokio::join!(async move { store_a.store(mem_a).await }, async move {
            store_b.store(mem_b).await
        },);

        // The lock serializes the two stores. The underlying backend treats the
        // second store as a duplicate id, so at least one must succeed and the
        // memory must end up in exactly one tier.
        assert!(
            res_a.is_ok() || res_b.is_ok(),
            "at least one concurrent store should succeed: {:?}, {:?}",
            res_a,
            res_b
        );

        // The memory must exist in exactly one tier.
        let mut count = 0usize;
        for tier in [
            MemoryTier::Working,
            MemoryTier::ShortTerm,
            MemoryTier::LongTerm,
            MemoryTier::Archival,
        ] {
            let backend: &dyn MemoryStore = match tier {
                MemoryTier::Working => &store.working,
                MemoryTier::ShortTerm => &store.short_term,
                MemoryTier::LongTerm => &store.long_term,
                MemoryTier::Archival => &store.archival,
            };
            if backend.get(&id).await.unwrap().is_some() {
                count += 1;
            }
        }
        assert_eq!(count, 1, "memory should exist in exactly one backend after concurrent stores");

        // Search must return exactly one copy.
        let results = store
            .search(MemoryQuery::new().for_user("u1").limit(10))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
    }
}
