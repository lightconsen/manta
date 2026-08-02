//! Effectiveness tracking and tier adjustments for recalled memories.

use super::*;

/// Compute a stable id for effectiveness tracking.
///
/// Hybrid and QMD results are constructed with synthetic `MemoryId`s, so using
/// `mem.id` for effectiveness tracking creates a new record on every recall.
/// For those sources we fall back to a content hash (scoped by user) so the
/// same recalled content aggregates into one effectiveness record.
pub(super) fn effectiveness_tracking_id(mem: &Memory, user_id: &str) -> String {
    const SYNTHETIC_TYPES: &[&str] = &["semantic", "session", "hybrid", "qmd"];
    if SYNTHETIC_TYPES.contains(&mem.memory_type.as_str()) {
        let mut hasher = Sha256::new();
        hasher.update(user_id.as_bytes());
        hasher.update(mem.content.as_bytes());
        format!("recall-content-{}", hex::encode(hasher.finalize()))
    } else {
        mem.id.to_string()
    }
}

impl MemoryManager {
    /// Evaluate whether recently-recalled memories were "hit" by the LLM
    /// response.
    ///
    /// For each recent recall in `session_key`, checks if `response_text`
    /// contains a significant substring of the recalled memory content. If
    /// so, marks it as a hit in the effectiveness tracker.
    ///
    /// This should be called immediately after `get_completion()` returns.
    pub async fn evaluate_response_hits(&self, session_key: &str, response_text: &str) {
        let effectiveness = match self.effectiveness {
            Some(ref e) => e.clone(),
            None => return,
        };

        let recalls_to_evaluate = {
            let mut guard = self.recent_recalls.write().await;
            guard.remove(session_key)
        };

        let Some(recalls) = recalls_to_evaluate else {
            return;
        };

        let response_lower = response_text.to_lowercase();

        for recall in recalls {
            let probe = recall
                .memory_content
                .chars()
                .take(80)
                .collect::<String>()
                .to_lowercase();
            if probe.len() < 3 {
                // Too short to meaningfully match; skip
                continue;
            }
            if response_lower.contains(&probe) {
                effectiveness.mark_hit(&recall.recall_id).await;
            }
        }
    }

    /// Evaluate recalled memories for effectiveness and adjust importance
    /// scores.
    ///
    /// Closes the feedback loop: uses the effectiveness tracker to evaluate
    /// memories that have been recalled recently, and adjusts their importance
    /// scores based on hit rates.
    ///
    /// Rate-limited: skips if adjustments were applied within the last 5
    /// minutes.
    pub async fn apply_effectiveness_adjustments(&self) {
        let effectiveness = match &self.effectiveness {
            Some(e) => e.clone(),
            None => return,
        };

        // Rate limit: skip if adjustments were applied within the last 5 minutes.
        // Acquire a write lock and update the timestamp before doing any work so
        // concurrent callers cannot all pass the check and run adjustments in
        // parallel.
        {
            let mut guard = self.last_adjustment.write().await;
            if let Some(last) = *guard {
                if last.elapsed().as_secs() < 300 {
                    return;
                }
            }
            *guard = Some(std::time::Instant::now());
        }

        // Collect memory IDs that have been tracked by effectiveness
        let Some(memory_ids) = self.collect_tracked_memory_ids().await else {
            return;
        };

        if memory_ids.is_empty() {
            return;
        }

        let mut adjusted = 0usize;
        let mut migrated = 0usize;

        for memory_id in memory_ids {
            // Hold an independent clone of the store Arc so the tiered-store
            // reference and its per-memory lock guard do not borrow `self`.
            let store_arc = Arc::clone(&self.store);
            let tiered_store = store_arc.as_tiered_store();

            let (was_adjusted, was_migrated) = if let Some(tiered) = tiered_store {
                let _guard = tiered.lock_memory(&memory_id).await;
                self.apply_adjustment_for_memory(&memory_id, &effectiveness, Some(tiered))
                    .await
            } else {
                self.apply_adjustment_for_memory(&memory_id, &effectiveness, None)
                    .await
            };

            if was_adjusted {
                adjusted += 1;
            }
            if was_migrated {
                migrated += 1;
            }
        }

        if adjusted > 0 {
            info!("Applied {} effectiveness adjustments", adjusted);
        }
        if migrated > 0 {
            info!("Migrated {} memories based on effectiveness", migrated);
        }
    }

    /// Apply a single effectiveness adjustment atomically.
    ///
    /// If `tiered_store` is provided, the caller must already hold the
    /// per-memory lock so the get/evaluate/update/migrate sequence cannot be
    /// interleaved with other mutating operations on the same memory.
    async fn apply_adjustment_for_memory(
        &self,
        memory_id: &str,
        effectiveness: &Arc<EffectivenessTracker>,
        tiered_store: Option<&TieredStore>,
    ) -> (bool, bool) {
        // Get current memory to read importance score
        let Ok(Some(memory)) = self
            .store
            .get(&crate::memory::MemoryId::new(memory_id))
            .await
        else {
            return (false, false);
        };

        let action = effectiveness
            .evaluate(memory_id, memory.importance_score)
            .await;
        if action == crate::memory::effectiveness::EffectivenessAction::NoOp {
            return (false, false);
        }

        let old_score = memory.importance_score;
        let new_score = effectiveness.apply_action(action, old_score);
        if (new_score - old_score).abs() < 0.001 {
            return (false, false);
        }

        let updated_result = if let Some(tiered) = tiered_store {
            tiered
                .update_importance_score_unlocked(
                    &crate::memory::MemoryId::new(memory_id),
                    new_score,
                )
                .await
        } else {
            self.store
                .update_importance_score(&crate::memory::MemoryId::new(memory_id), new_score)
                .await
        };

        let updated = match updated_result {
            Ok(Some(updated)) => updated,
            Ok(None) => {
                debug!(
                    "Skipping effectiveness update for {}: memory removed concurrently",
                    memory_id
                );
                return (false, false);
            }
            Err(crate::error::SyscityError::NotFound { .. }) => {
                debug!("Skipping effectiveness update for {}: memory no longer exists", memory_id);
                return (false, false);
            }
            Err(e) => {
                warn!("Failed to update importance score for {}: {}", memory_id, e);
                return (false, false);
            }
        };

        info!(
            "Effectiveness adjustment: memory {} importance {:.3} -> {:.3}",
            memory_id, old_score, new_score
        );

        // Closed-loop tier migration: if the store is tiered, re-evaluate
        // using effectiveness statistics and migrate when the evaluator
        // recommends a direct promotion/demotion based on hit rate.
        let mut was_migrated = false;
        if let (Some(ref tier_index), Some(tiered)) = (&self.tier_index, tiered_store) {
            let Some(tiered_meta) = tier_index.get(memory_id) else {
                return (true, false);
            };
            let Some(stats) = effectiveness.memory_stats(memory_id).await else {
                return (true, false);
            };

            let tier_action = tiered
                .evaluator()
                .evaluate(&updated, &tiered_meta, Some(&stats));

            if let TierAction::Promote(target) | TierAction::Demote(target) = tier_action {
                if let Err(e) = tiered.migrate_memory_unlocked(&updated, target).await {
                    warn!(
                        "Effectiveness-driven migration failed for {} to {}: {}",
                        memory_id, target, e
                    );
                    return (true, false);
                }

                let from_level = tiered_meta.tier.to_string();
                let to_level = target.to_string();
                info!(
                    "Effectiveness migration: memory {} {} -> {}",
                    memory_id, from_level, to_level
                );

                if matches!(tier_action, TierAction::Promote(_)) {
                    effectiveness.record_promotion(memory_id).await;
                } else {
                    effectiveness.record_demotion(memory_id).await;
                }

                if let Some(ref event_log) = self.event_log {
                    let event = MemoryEventBuilder::new().promotion(
                        "effectiveness",
                        format!("promo-{}", uuid::Uuid::new_v4()),
                        from_level,
                        to_level,
                        "effectiveness",
                    );
                    if let Err(e) = event_log.append(&event).await {
                        warn!("Failed to append promotion event: {}", e);
                    }
                }

                was_migrated = true;
            }
        }

        (true, was_migrated)
    }

    /// Collect memory IDs that have been tracked by the effectiveness system.
    async fn collect_tracked_memory_ids(&self) -> Option<Vec<String>> {
        let effectiveness = self.effectiveness.as_ref()?;

        // Get top and under performers that qualify for adjustment
        let mut ids = Vec::new();
        for (id, _stats) in effectiveness.top_performers(50).await {
            ids.push(id);
        }
        for (id, _stats) in effectiveness.under_performers(50).await {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }

        Some(ids)
    }
}
