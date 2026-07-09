//! Effectiveness Closed-Loop Integration Tests
//!
//! Verifies that when EffectivenessTracker adjusts a memory's importance_score,
//! TieredStore::update() automatically migrates the memory to the correct tier
//! backend.  This is the core of the "Effectiveness 闭环反馈" feature.

use syscity::memory::{
    EffectivenessAction, EffectivenessConfig, EffectivenessTracker, Memory, MemoryStore,
    MemoryTier, TieredStore,
};

/// Simulate recall events with configurable hit rates.
async fn simulate_recalls(
    tracker: &EffectivenessTracker,
    memory_id: &str,
    session_key: &str,
    count: usize,
    hit: bool,
    importance: f32,
) {
    for i in 0..count {
        let recall_id = format!("recall-{}-{}", memory_id, i);
        tracker
            .record_recall(&recall_id, memory_id, session_key, "fact", importance, 0)
            .await;
        if hit {
            tracker.mark_hit(&recall_id).await;
        }
    }
}

#[tokio::test]
async fn effectiveness_boost_triggers_tier_promotion() {
    let store = TieredStore::new_in_memory().await.unwrap();

    // 1. Store a low-importance memory → Working tier
    let mem = Memory::new("u1", "Promote via effectiveness", "fact").with_importance_score(0.1);
    let id = store.store(mem.clone()).await.unwrap();
    assert_eq!(
        store.tier_index().get_tier(&id.0),
        Some(MemoryTier::Working),
        "low-importance memory should start in Working"
    );

    // Bump access count so promotion criteria are met
    store.tier_index().record_access(&id.0);
    store.tier_index().record_access(&id.0);
    store.tier_index().record_access(&id.0);

    // 2. Create tracker configured for quick boosting
    let tracker = EffectivenessTracker::new(EffectivenessConfig {
        auto_adjust: true,
        promotion_threshold: 0.7,
        demotion_threshold: 0.2,
        min_recalls_for_adjustment: 3,
        importance_boost: 0.2,
        importance_penalty: 0.1,
        max_importance: 1.0,
        min_importance: 0.0,
        promote_directly_threshold: 0.9,
        demote_directly_threshold: 0.1,
        max_events_per_memory: 1000,
        max_tracked_memories: 50_000,
    });

    // 3. Simulate 3 recalls, all hits → hit_rate = 1.0 > 0.7
    simulate_recalls(&tracker, &id.0, "u1:conv1", 3, true, 0.1).await;

    // 4. Evaluate and apply boost
    let action = tracker.evaluate(&id.0, 0.1).await;
    assert_eq!(action, EffectivenessAction::Boost, "3/3 hits should trigger Boost");

    let boosted_score = tracker.apply_action(action, 0.1);
    assert!(
        (boosted_score - 0.3).abs() < 0.001,
        "expected importance 0.3, got {}",
        boosted_score
    );

    // 5. Update memory via TieredStore — should trigger auto-migration
    let mut updated = mem.clone();
    updated.id = id.clone();
    updated.importance_score = boosted_score;
    store.update(updated.clone()).await.unwrap();

    // 6. Verify tier promotion: Working → ShortTerm
    assert_eq!(
        store.tier_index().get_tier(&id.0),
        Some(MemoryTier::ShortTerm),
        "boosted memory should be promoted to ShortTerm"
    );

    // 7. Verify data is retrievable and consistent
    let fetched = store.get(&id).await.unwrap().unwrap();
    assert_eq!(fetched.content, "Promote via effectiveness");
    assert!(
        (fetched.importance_score - 0.3).abs() < 0.001,
        "fetched importance should match updated score"
    );

    // 8. Second boost cycle: ShortTerm → LongTerm
    simulate_recalls(&tracker, &id.0, "u1:conv2", 3, true, 0.3).await;

    let action2 = tracker.evaluate(&id.0, 0.3).await;
    assert_eq!(action2, EffectivenessAction::Boost, "continued hits should trigger Boost again");

    let boosted_score2 = tracker.apply_action(action2, 0.3);
    assert!(
        (boosted_score2 - 0.5).abs() < 0.001,
        "expected importance 0.5, got {}",
        boosted_score2
    );

    updated.importance_score = boosted_score2;
    store.update(updated).await.unwrap();

    assert_eq!(
        store.tier_index().get_tier(&id.0),
        Some(MemoryTier::LongTerm),
        "double-boosted memory should reach LongTerm"
    );

    let fetched2 = store.get(&id).await.unwrap().unwrap();
    assert!(
        (fetched2.importance_score - 0.5).abs() < 0.001,
        "fetched importance should reflect LongTerm score"
    );
}

#[tokio::test]
async fn effectiveness_penalty_triggers_tier_demotion() {
    let store = TieredStore::new_in_memory().await.unwrap();

    // 1. Store a high-importance memory → LongTerm tier
    let mem = Memory::new("u1", "Demote via effectiveness", "fact").with_importance_score(0.8);
    let id = store.store(mem.clone()).await.unwrap();
    assert_eq!(
        store.tier_index().get_tier(&id.0),
        Some(MemoryTier::LongTerm),
        "high-importance memory should start in LongTerm"
    );

    // 2. Create tracker configured for quick penalising
    let tracker = EffectivenessTracker::new(EffectivenessConfig {
        auto_adjust: true,
        promotion_threshold: 0.7,
        demotion_threshold: 0.2,
        min_recalls_for_adjustment: 3,
        importance_boost: 0.1,
        importance_penalty: 0.4,
        max_importance: 1.0,
        min_importance: 0.0,
        promote_directly_threshold: 0.9,
        demote_directly_threshold: 0.1,
        max_events_per_memory: 1000,
        max_tracked_memories: 50_000,
    });

    // 3. Simulate 3 recalls, all misses → hit_rate = 0.0 < 0.2
    simulate_recalls(&tracker, &id.0, "u1:conv1", 3, false, 0.8).await;

    // 4. Evaluate and apply penalty
    let action = tracker.evaluate(&id.0, 0.8).await;
    assert_eq!(action, EffectivenessAction::Penalize, "0/3 hits should trigger Penalize");

    let penalised_score = tracker.apply_action(action, 0.8);
    assert!(
        (penalised_score - 0.4).abs() < 0.001,
        "expected importance 0.4, got {}",
        penalised_score
    );

    // 5. Update memory via TieredStore — should trigger auto-demigration
    let mut updated = mem.clone();
    updated.id = id.clone();
    updated.importance_score = penalised_score;
    store.update(updated.clone()).await.unwrap();

    // 6. Verify tier demotion: LongTerm → ShortTerm
    assert_eq!(
        store.tier_index().get_tier(&id.0),
        Some(MemoryTier::ShortTerm),
        "penalised memory should be demoted to ShortTerm"
    );

    // 7. Verify data consistency
    let fetched = store.get(&id).await.unwrap().unwrap();
    assert_eq!(fetched.content, "Demote via effectiveness");
    assert!(
        (fetched.importance_score - 0.4).abs() < 0.001,
        "fetched importance should match penalised score"
    );

    // 8. Second penalty cycle: ShortTerm → Working
    simulate_recalls(&tracker, &id.0, "u1:conv2", 3, false, 0.4).await;

    let action2 = tracker.evaluate(&id.0, 0.4).await;
    assert_eq!(
        action2,
        EffectivenessAction::Penalize,
        "continued misses should trigger Penalize again"
    );

    let penalised_score2 = tracker.apply_action(action2, 0.4);
    assert!(
        (penalised_score2 - 0.0).abs() < 0.001,
        "expected importance 0.0, got {}",
        penalised_score2
    );

    updated.importance_score = penalised_score2;
    store.update(updated).await.unwrap();

    assert_eq!(
        store.tier_index().get_tier(&id.0),
        Some(MemoryTier::Working),
        "double-penalised memory should reach Working"
    );

    let fetched2 = store.get(&id).await.unwrap().unwrap();
    assert!(
        (fetched2.importance_score - 0.0).abs() < 0.001,
        "fetched importance should reflect Working score"
    );
}

#[tokio::test]
async fn effectiveness_noop_preserves_tier() {
    let store = TieredStore::new_in_memory().await.unwrap();

    // Store a medium-importance memory → ShortTerm tier
    let mem = Memory::new("u1", "Stable tier memory", "fact").with_importance_score(0.4);
    let id = store.store(mem.clone()).await.unwrap();
    assert_eq!(store.tier_index().get_tier(&id.0), Some(MemoryTier::ShortTerm));

    let tracker = EffectivenessTracker::new(EffectivenessConfig {
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
    });

    // Simulate 3 recalls, 1 hit → hit_rate = 0.33, between thresholds
    simulate_recalls(&tracker, &id.0, "u1:conv1", 3, false, 0.4).await;
    // Manually mark one hit (recall_id follows format! "recall-{memory_id}-{i}")
    tracker.mark_hit(&format!("recall-{}-0", id.0)).await;

    let action = tracker.evaluate(&id.0, 0.4).await;
    assert_eq!(action, EffectivenessAction::NoOp, "mixed hit rate should yield NoOp");

    let new_score = tracker.apply_action(action, 0.4);
    assert!((new_score - 0.4).abs() < 0.001, "NoOp should preserve importance");

    let mut updated = mem.clone();
    updated.id = id.clone();
    updated.importance_score = new_score;
    store.update(updated).await.unwrap();

    // Tier should remain unchanged
    assert_eq!(
        store.tier_index().get_tier(&id.0),
        Some(MemoryTier::ShortTerm),
        "NoOp should preserve tier"
    );

    let fetched = store.get(&id).await.unwrap().unwrap();
    assert_eq!(fetched.importance_score, 0.4);
}

#[tokio::test]
async fn effectiveness_data_consistency_across_backends() {
    let store = TieredStore::new_in_memory().await.unwrap();

    // Start in Working
    let mem = Memory::new("u1", "Consistency test", "fact").with_importance_score(0.1);
    let id = store.store(mem.clone()).await.unwrap();
    store.tier_index().record_access(&id.0);
    store.tier_index().record_access(&id.0);
    store.tier_index().record_access(&id.0);

    let tracker = EffectivenessTracker::new(EffectivenessConfig {
        auto_adjust: true,
        promotion_threshold: 0.7,
        demotion_threshold: 0.2,
        min_recalls_for_adjustment: 3,
        importance_boost: 0.5, /* big jump: Working → LongTerm in one go? No — evaluate checks
                                * only next tier */
        importance_penalty: 0.1,
        max_importance: 1.0,
        min_importance: 0.0,
        promote_directly_threshold: 0.9,
        demote_directly_threshold: 0.1,
        max_events_per_memory: 1000,
        max_tracked_memories: 50_000,
    });

    // 3 hits
    simulate_recalls(&tracker, &id.0, "u1:conv1", 3, true, 0.1).await;

    let action = tracker.evaluate(&id.0, 0.1).await;
    assert_eq!(action, EffectivenessAction::Boost);

    let new_score = tracker.apply_action(action, 0.1);
    let mut updated = mem.clone();
    updated.id = id.clone();
    updated.importance_score = new_score;
    store.update(updated.clone()).await.unwrap();

    // Should be in ShortTerm (one tier at a time)
    let current_tier = store.tier_index().get_tier(&id.0).unwrap();

    // Verify the memory is ONLY in the current backend, not in old ones
    for tier in [
        MemoryTier::Working,
        MemoryTier::ShortTerm,
        MemoryTier::LongTerm,
        MemoryTier::Archival,
    ] {
        let exists = store.tier_index().ids_in_tier(tier).contains(&id.0);
        if tier == current_tier {
            assert!(exists, "memory should be in current tier {:?}", tier);
        } else {
            assert!(!exists, "memory should NOT remain in old tier {:?}", tier);
        }
    }

    // Search should still find it
    let results = store
        .search(syscity::memory::MemoryQuery::new().for_user("u1").limit(10))
        .await
        .unwrap();
    let found = results.iter().any(|m| m.id.0 == id.0);
    assert!(found, "search should still find migrated memory");
}

#[tokio::test]
async fn tiered_store_survives_process_restart() {
    let temp_dir =
        std::env::temp_dir().join(format!("syscity_tier_crash_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    // Create some memories in persistent tiers (Working tier uses InMemoryStore and
    // won't persist)
    let memory_ids = {
        let store = TieredStore::new(&temp_dir).await.unwrap();

        // ShortTerm tier (persistent)
        let mem1 = Memory::new("u1", "Should be in ShortTerm", "fact").with_importance_score(0.3);
        let id1 = store.store(mem1).await.unwrap();

        // ShortTerm tier (another)
        let mem2 =
            Memory::new("u1", "Should also be in ShortTerm", "fact").with_importance_score(0.5);
        let id2 = store.store(mem2).await.unwrap();

        // LongTerm tier
        let mem3 = Memory::new("u1", "Should be in LongTerm", "fact").with_importance_score(0.8);
        let id3 = store.store(mem3).await.unwrap();

        // LongTerm tier (another)
        let mem4 =
            Memory::new("u1", "Should also be in LongTerm", "fact").with_importance_score(0.9);
        let id4 = store.store(mem4).await.unwrap();

        store.close().await.unwrap();

        (id1, id2, id3, id4)
    };

    // Simulate process restart: create a new store instance with the same base
    // directory
    let store2 = TieredStore::new(&temp_dir).await.unwrap();

    // Verify all memories are retrievable
    let fetched1 = store2.get(&memory_ids.0).await.unwrap().unwrap();
    assert_eq!(fetched1.content, "Should be in ShortTerm");

    let fetched2 = store2.get(&memory_ids.1).await.unwrap().unwrap();
    assert_eq!(fetched2.content, "Should also be in ShortTerm");

    let fetched3 = store2.get(&memory_ids.2).await.unwrap().unwrap();
    assert_eq!(fetched3.content, "Should be in LongTerm");

    let fetched4 = store2.get(&memory_ids.3).await.unwrap().unwrap();
    assert_eq!(fetched4.content, "Should also be in LongTerm");

    // Verify search still works
    let results = store2
        .search(syscity::memory::MemoryQuery::new().for_user("u1").limit(10))
        .await
        .unwrap();
    assert_eq!(results.len(), 4, "search should find all 4 memories");

    // Cleanup
    store2.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn tiered_store_recovers_from_crash_duplicates() {
    let temp_dir =
        std::env::temp_dir().join(format!("syscity_tier_crash2_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    // First, create a store with a memory in ShortTerm
    let id = {
        let store = TieredStore::new(&temp_dir).await.unwrap();

        // Store in ShortTerm
        let mem =
            Memory::new("u1", "Memory that gets duplicated", "fact").with_importance_score(0.4);
        let id = store.store(mem).await.unwrap();

        store.close().await.unwrap();
        id
    };

    // Now, let's create a new store, get the memory, and manually store it in
    // LongTerm too using the DatabaseStore directly
    let stored_mem = {
        let store2 = TieredStore::new(&temp_dir).await.unwrap();
        let mem = store2.get(&id).await.unwrap().unwrap();

        // Create a new DatabaseStore for long_term.db directly and store it there too
        let long_term_db_path = temp_dir.join("long_term.db");
        let long_term_store = syscity::memory::DatabaseStore::new(&format!(
            "sqlite://{}",
            long_term_db_path.to_string_lossy()
        ))
        .await
        .unwrap();
        long_term_store.store(mem.clone()).await.unwrap();

        store2.close().await.unwrap();
        mem
    };

    // Now create a new store instance — it should recover from the duplicate when
    // migrate_memory is called
    let store3 = TieredStore::new(&temp_dir).await.unwrap();

    // Verify memory is retrievable
    let fetched = store3.get(&id).await.unwrap().unwrap();
    assert_eq!(fetched.content, "Memory that gets duplicated");

    // Now explicitly migrate it to trigger the idempotent recovery path
    store3
        .migrate_memory(&stored_mem, MemoryTier::LongTerm)
        .await
        .unwrap();

    // Verify it's now in LongTerm
    let current_tier = store3.tier_index().get_tier(&id.0).unwrap();
    assert_eq!(current_tier, MemoryTier::LongTerm, "memory should be in LongTerm");

    // Cleanup
    store3.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn tiered_store_recovers_missing_index() {
    let temp_dir =
        std::env::temp_dir().join(format!("syscity_tier_crash3_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    // Store a memory, then delete the index to simulate index loss
    let id = {
        let store = TieredStore::new(&temp_dir).await.unwrap();

        let mem =
            Memory::new("u1", "Memory that survives index loss", "fact").with_importance_score(0.5);
        let id = store.store(mem).await.unwrap();

        store.close().await.unwrap();

        // Delete the index file
        std::fs::remove_file(&temp_dir.join(syscity::memory::TIER_INDEX_FILE_NAME)).unwrap();

        id
    };

    // Create new store instance - it should fall back to empty index but still find
    // the memory via scan
    let store2 = TieredStore::new(&temp_dir).await.unwrap();

    // Memory should still be retrievable (via fallback scan when index misses)
    let fetched = store2.get(&id).await.unwrap().unwrap();
    assert_eq!(fetched.content, "Memory that survives index loss");

    // And after get(), index should be updated
    assert_eq!(
        store2.tier_index().get_tier(&id.0),
        Some(MemoryTier::ShortTerm),
        "index should be rebuilt after get()"
    );

    // Cleanup
    store2.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&temp_dir);
}
