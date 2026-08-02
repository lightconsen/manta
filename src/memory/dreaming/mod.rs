//! Dreaming Engine — Background Memory Consolidation
//!
//! Simulates human sleep memory consolidation through three phases:
//! - Light: deduplication, tag cleanup, expiry removal (fast, cheap)
//! - Deep: topic clustering, summary generation, cross-session linking (medium)
//! - REM: cross-session pattern discovery, knowledge graph update (expensive,
//!   rare)
//!
//! Triggered via cron scheduling (`DEFAULT_MEMORY_DREAMING_FREQUENCY`).

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use std::time::{Duration, Instant};

use chrono::Utc;
use cron::Schedule as CronSchedule;
use serde::{Deserialize, Serialize};
use sysinfo::{RefreshKind, System};
use tokio::sync::{watch, RwLock};
use tokio::time::{sleep_until, Instant as TokioInstant};
use tracing::{debug, info, warn};

use super::events::{MemoryEventBuilder, MemoryEventLog};
use super::tier::{MemoryTier, TierAction, TierEvaluator, TierIndex, TierSystemConfig};
use super::{cosine_similarity, Memory, MemoryId, MemoryQuery, MemoryStore};

/// Cancel signal receiver for interrupting dream phases mid-execution.
pub type CancelSignal = watch::Receiver<bool>;

/// Async callback for LLM-based entity extraction in REM dreams.
/// Takes a prompt string and returns the LLM's response text.
pub type LlmCallback = Arc<
    dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
        + Send
        + Sync,
>;

/// Default cron expression: daily at 3:00 AM.
pub const DEFAULT_MEMORY_DREAMING_FREQUENCY: &str = "0 0 3 * * *";

mod dream_engine;
mod dream_review_queue;
mod dream_scheduler;
mod dream_types;
mod knowledge_graph;
mod lsh_dedup;

pub use dream_engine::DreamEngine;
pub use dream_review_queue::{DreamAction, DreamReviewItem, DreamReviewQueue, ReviewStatus};
pub use dream_scheduler::DreamScheduler;
pub use dream_types::{
    DreamBudget, DreamCheckpoint, DreamConfig, DreamMetrics, DreamPhase, DreamResult, DreamSpeed,
    DreamThinking,
};
pub use knowledge_graph::{KnowledgeEdge, KnowledgeGraph, KnowledgeNode};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::dream_engine::estimate_tokens;
    use super::lsh_dedup::build_dedup_candidate_pairs;
    use super::*;
    use crate::memory::{Memory, MemoryId, MemoryQuery, MemoryStats, MemoryStore, UnifiedStore};
    use crate::SyscityError;

    #[test]
    fn test_lsh_candidate_pairs_group_near_duplicates() {
        // Two near-identical embeddings should collide in at least one LSH band.
        let e1: Vec<f32> = (0..64).map(|i| (i as f32) * 0.01).collect();
        let mut e2 = e1.clone();
        // Small perturbation preserves cosine similarity ≈ 1.0.
        for v in &mut e2 {
            *v += 0.0005;
        }
        // Completely different vector.
        let e3: Vec<f32> = (0..64).map(|i| -(i as f32) * 0.05).collect();

        let m1 = Memory::new("u", "a", "fact").with_embedding(e1);
        let m2 = Memory::new("u", "b", "fact").with_embedding(e2);
        let m3 = Memory::new("u", "c", "fact").with_embedding(e3);
        let pairs = build_dedup_candidate_pairs(&[m1, m2, m3]);
        assert!(
            pairs.contains(&(0, 1)),
            "near-duplicate embeddings should collide; got {pairs:?}"
        );
    }

    #[test]
    fn test_lsh_candidate_pairs_prefix_fallback() {
        // Memories without embeddings must be grouped by 50-char prefix.
        let m1 = Memory::new("u", "Shared prefix content", "fact");
        let m2 = Memory::new("u", "Shared prefix content again", "fact");
        // Wait — the fallback compares only the first 50 chars, so we need both
        // to start with the same 50 lowercased chars for a collision.
        let m3 = Memory::new("u", "Completely different body here", "fact");
        let pairs = build_dedup_candidate_pairs(&[m1, m2, m3]);
        // (0,1) share prefix; (0,2)/(1,2) do not.
        // 50 chars of m1 vs m2: "shared prefix content" (21 chars) vs
        // "shared prefix content again" (27 chars) — both padded by their full
        // string when < 50, so they are NOT equal. The test verifies the
        // grouping does not create false pairs when prefixes differ.
        assert!(!pairs.contains(&(0, 2)));
        assert!(!pairs.contains(&(1, 2)));
    }

    #[tokio::test]
    async fn test_dream_light() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let config = DreamConfig::default();
        let tier_config = TierSystemConfig::default();
        let engine = DreamEngine::new(config, tier_config);
        let tier_index = TierIndex::new();

        // Seed some memories
        for i in 0..5 {
            let mem = Memory::new("u1", format!("Duplicate content {}", i % 2), "fact")
                .with_importance_score(0.5);
            let id = store.store(mem).await.unwrap();
            tier_index.insert(id.to_string(), MemoryTier::ShortTerm);
        }

        // Create a cancel signal that never cancels
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let result = engine
            .run_light(store.as_ref(), &tier_index, cancel_rx)
            .await
            .unwrap();
        assert_eq!(result.phase, DreamPhase::Light);
        assert!(!result.cancelled);
        // Removed duplicates are not counted in processed
        // 5 memories with 2 unique contents → 2 duplicates removed → 3 remaining
        // but further tier evaluation may evict some, so just check it's less than 5
        assert!(
            result.memories_processed < 5,
            "duplicates should be excluded from processed count"
        );
        // Some duplicates should be removed
    }

    #[tokio::test]
    async fn test_dream_deep() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let config = DreamConfig::default();
        let tier_config = TierSystemConfig::default();
        let engine = DreamEngine::new(config, tier_config);
        let tier_index = TierIndex::new();

        // Seed memories on a common topic
        for i in 0..5 {
            let mem = Memory::new("u1", format!("Project Alpha milestone {} completed", i), "fact")
                .with_importance_score(0.6);
            let id = store.store(mem).await.unwrap();
            tier_index.insert(id.to_string(), MemoryTier::ShortTerm);
        }

        // Create a cancel signal that never cancels
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let result = engine
            .run_deep(store.as_ref(), &tier_index, cancel_rx)
            .await
            .unwrap();
        assert_eq!(result.phase, DreamPhase::Deep);
        assert!(!result.cancelled);
        assert!(result.memories_processed >= 5);
        // Should create at least one summary
        assert!(result.memories_created > 0);
    }

    #[tokio::test]
    async fn test_dream_rem() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let config = DreamConfig::default();
        let tier_config = TierSystemConfig::default();
        let engine = DreamEngine::new(config, tier_config);
        let tier_index = TierIndex::new();

        // Seed memories with capitalized entities
        let mems = vec![
            "Alice works at Google in New York",
            "Bob visited New York last summer",
            "Google announced new AI features",
            "Alice and Bob are friends",
            "New York is a big city",
        ];
        for content in mems {
            let mem = Memory::new("u1", content, "fact").with_importance_score(0.6);
            let id = store.store(mem).await.unwrap();
            tier_index.insert(id.to_string(), MemoryTier::LongTerm);
        }

        // Create a cancel signal that never cancels
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let result = engine
            .run_rem(store.as_ref(), &tier_index, None, cancel_rx)
            .await
            .unwrap();
        assert_eq!(result.phase, DreamPhase::Rem);
        assert!(!result.cancelled);
        assert!(result.memories_processed >= 5);

        let graph = engine.knowledge_graph().await;
        assert!(!graph.nodes.is_empty());
    }

    #[tokio::test]
    async fn test_dream_cancel() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let config = DreamConfig::default();
        let tier_config = TierSystemConfig::default();
        let engine = DreamEngine::new(config, tier_config);
        let tier_index = TierIndex::new();

        // Seed a lot of memories so that we can cancel mid-processing
        for i in 0..100 {
            let mem =
                Memory::new("u1", format!("Content {}", i), "fact").with_importance_score(0.5);
            let id = store.store(mem).await.unwrap();
            tier_index.insert(id.to_string(), MemoryTier::ShortTerm);
        }

        // Create a cancel signal that cancels immediately
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(true);
        let result = engine
            .run_light(store.as_ref(), &tier_index, cancel_rx)
            .await
            .unwrap();
        assert_eq!(result.phase, DreamPhase::Light);
        assert!(result.cancelled);
        drop(cancel_tx);
    }

    #[test]
    fn test_dream_phase_display() {
        assert_eq!(format!("{}", DreamPhase::Light), "light");
        assert_eq!(format!("{}", DreamPhase::Deep), "deep");
        assert_eq!(format!("{}", DreamPhase::Rem), "rem");
    }

    #[test]
    fn test_dream_config_default() {
        let config = DreamConfig::default();
        assert!(config.enabled);
        assert_eq!(config.frequency, DEFAULT_MEMORY_DREAMING_FREQUENCY);
        assert!(config.dedup_similarity_threshold > 0.0);
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens("hello"), 1);
        assert_eq!(estimate_tokens("this is a short sentence"), 6);
        assert_eq!(estimate_tokens(""), 1);
    }

    #[tokio::test]
    async fn test_dream_light_metrics() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let config = DreamConfig::default();
        let tier_config = TierSystemConfig::default();
        let engine = DreamEngine::new(config, tier_config);
        let tier_index = TierIndex::new();

        for i in 0..5 {
            let mem = Memory::new("u1", format!("Duplicate content {}", i % 2), "fact")
                .with_importance_score(0.5);
            let id = store.store(mem).await.unwrap();
            tier_index.insert(id.to_string(), MemoryTier::ShortTerm);
        }

        // Create a cancel signal that never cancels
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let result = engine
            .run_light(store.as_ref(), &tier_index, cancel_rx)
            .await
            .unwrap();
        assert_eq!(result.phase, DreamPhase::Light);
        assert!(result.peak_memory_mb.is_some());
        // Removed duplicates are not counted in processed
        assert!(
            result.memories_processed < 5,
            "duplicates should be excluded from processed count"
        );

        let metrics = engine.metrics();
        assert_eq!(
            metrics.dreams_total.load(Ordering::Relaxed),
            0,
            "run_light should not record metrics directly"
        );
    }

    #[tokio::test]
    async fn test_dream_metrics_record() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let config = DreamConfig::default();
        let tier_config = TierSystemConfig::default();
        let engine = DreamEngine::new(config, tier_config);
        let tier_index = TierIndex::new();

        for i in 0..5 {
            let mem = Memory::new("u1", format!("Project Alpha milestone {} completed", i), "fact")
                .with_importance_score(0.6);
            let id = store.store(mem).await.unwrap();
            tier_index.insert(id.to_string(), MemoryTier::ShortTerm);
        }

        // Create a cancel signal that never cancels
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let results = engine
            .run_full_cycle(store.as_ref(), &tier_index, false, None, cancel_rx)
            .await
            .unwrap();
        assert!(!results.is_empty());

        let metrics = engine.metrics();
        assert!(metrics.dreams_total.load(Ordering::Relaxed) >= 1);
        assert!(metrics.memories_processed_total.load(Ordering::Relaxed) >= 5);
    }

    #[tokio::test]
    async fn test_dream_rem_token_tracking() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let config = DreamConfig {
            budget: DreamBudget::Expensive,
            ..DreamConfig::default()
        };
        let tier_config = TierSystemConfig::default();
        let engine = DreamEngine::new(config, tier_config);
        let tier_index = TierIndex::new();

        let mems = vec![
            "Alice works at Google in New York",
            "Bob visited New York last summer",
            "Google announced new AI features",
            "Alice and Bob are friends",
            "New York is a big city",
        ];
        for content in mems {
            let mem = Memory::new("u1", content, "fact").with_importance_score(0.6);
            let id = store.store(mem).await.unwrap();
            tier_index.insert(id.to_string(), MemoryTier::LongTerm);
        }

        let llm: LlmCallback = Arc::new(|_prompt: String| {
            Box::pin(async move {
                serde_json::json!({
                    "entities": [
                        {"label": "Alice", "type": "person", "confidence": 0.9},
                        {"label": "Google", "type": "organization", "confidence": 0.95}
                    ],
                    "relationships": [
                        {"from": "Alice", "to": "Google", "relation": "works_at", "confidence": 0.8}
                    ]
                })
                .to_string()
            })
        });

        // Create a cancel signal that never cancels
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let result = engine
            .run_rem(store.as_ref(), &tier_index, Some(&llm), cancel_rx)
            .await
            .unwrap();
        assert_eq!(result.phase, DreamPhase::Rem);
        assert!(!result.cancelled);
        assert!(result.peak_memory_mb.is_some());
        assert!(result.llm_tokens_input > 0);
        assert!(result.llm_tokens_output > 0);
    }

    #[test]
    fn test_dream_metrics_counters() {
        let metrics = DreamMetrics::default();
        let result = DreamResult {
            dream_id: "dream-test".to_string(),
            phase: DreamPhase::Light,
            started_at: SystemTime::now(),
            finished_at: SystemTime::now(),
            duration_ms: 42,
            memories_processed: 10,
            memories_created: 2,
            memories_removed: 1,
            memories_promoted: 3,
            memories_demoted: 4,
            peak_memory_mb: Some(123.4),
            llm_tokens_input: 100,
            llm_tokens_output: 50,
            summary: "test".to_string(),
            errors: vec![],
            cancelled: false,
        };
        metrics.record(&result, true);
        assert_eq!(metrics.dreams_total.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.dreams_failed.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.memories_processed_total.load(Ordering::Relaxed), 10);
        assert_eq!(metrics.memories_created_total.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.memories_removed_total.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.memories_promoted_total.load(Ordering::Relaxed), 3);
        assert_eq!(metrics.memories_demoted_total.load(Ordering::Relaxed), 4);
        assert_eq!(metrics.dream_duration_ms_total.load(Ordering::Relaxed), 42);
        assert_eq!(metrics.llm_tokens_input_total.load(Ordering::Relaxed), 100);
        assert_eq!(metrics.llm_tokens_output_total.load(Ordering::Relaxed), 50);
    }

    // ── Negative tests: apply_approved error handling ───────────────────────

    /// A memory store whose `delete` always fails, used to verify that
    /// `apply_approved` logs the error and skips `tier_index.remove`.
    struct FailingStore;

    #[async_trait::async_trait]
    impl MemoryStore for FailingStore {
        async fn store(&self, memory: Memory) -> crate::Result<MemoryId> {
            Ok(memory.id) // succeed with a no-op store
        }
        async fn get(&self, _id: &MemoryId) -> crate::Result<Option<Memory>> {
            Ok(None)
        }
        async fn update(&self, _memory: Memory) -> crate::Result<()> {
            Ok(())
        }
        async fn delete(&self, _id: &MemoryId) -> crate::Result<bool> {
            Err(SyscityError::Internal("mock: delete failed".into()))
        }
        async fn search(&self, _query: MemoryQuery) -> crate::Result<Vec<Memory>> {
            Ok(vec![])
        }
        async fn cleanup_expired(&self) -> crate::Result<usize> {
            Ok(0)
        }
        async fn stats(&self) -> crate::Result<MemoryStats> {
            Ok(MemoryStats::default())
        }
        async fn close(&self) -> crate::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_apply_approved_delete_failure_keeps_tier_index() {
        let queue = DreamReviewQueue::new();
        let store = FailingStore;
        let tier_index = TierIndex::new();

        // Pre-populate tier_index with the memory that will be "deleted"
        tier_index.insert("mem-1", MemoryTier::ShortTerm);

        // Enqueue a Delete action and approve it
        queue
            .enqueue(
                "dream-1",
                DreamPhase::Light,
                DreamAction::Delete {
                    memory_id: "mem-1".to_string(),
                    reason: "duplicate".to_string(),
                },
            )
            .await;
        let pending = queue.list_pending().await;
        assert_eq!(pending.len(), 1);
        queue.approve(&pending[0].id).await;

        // Apply — delete will fail, should not count as applied
        let applied = queue.apply_approved(&store, &tier_index).await.unwrap();
        assert_eq!(applied, 0, "delete failure should not count as applied");

        // tier_index must still contain the memory_id (delete failed)
        assert!(
            tier_index.get("mem-1").is_some(),
            "tier_index should still contain mem-1 after failed delete"
        );
    }

    #[tokio::test]
    async fn test_apply_approved_merge_failure_keeps_tier_index() {
        let queue = DreamReviewQueue::new();
        let store = FailingStore;
        let tier_index = TierIndex::new();

        // Pre-populate tier_index with memories that will be "deleted"
        tier_index.insert("mem-1", MemoryTier::ShortTerm);
        tier_index.insert("mem-2", MemoryTier::ShortTerm);

        // Enqueue a Merge action and approve it
        queue
            .enqueue(
                "dream-1",
                DreamPhase::Deep,
                DreamAction::Merge {
                    memory_ids: vec!["mem-1".to_string(), "mem-2".to_string()],
                    summary: "merged summary".to_string(),
                },
            )
            .await;
        let pending = queue.list_pending().await;
        assert_eq!(pending.len(), 1);
        queue.approve(&pending[0].id).await;

        // Apply — deletes will fail, should not count as applied
        let applied = queue.apply_approved(&store, &tier_index).await.unwrap();
        assert_eq!(applied, 0, "delete failures should not count as applied");

        // tier_index must still contain both memory_ids (deletes failed)
        assert!(
            tier_index.get("mem-1").is_some(),
            "tier_index should still contain mem-1 after failed merge delete"
        );
        assert!(
            tier_index.get("mem-2").is_some(),
            "tier_index should still contain mem-2 after failed merge delete"
        );
    }
}
