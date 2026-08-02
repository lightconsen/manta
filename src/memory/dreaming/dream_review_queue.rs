//! Human-in-the-loop review queue for dream-proposed actions.

use super::*;

// ─────────────────────────────────────────────────────────

/// Action proposed by a dream phase for human review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DreamAction {
    /// Delete a memory (e.g., duplicate, expired).
    Delete { memory_id: String, reason: String },
    /// Merge multiple memories into a summary.
    Merge {
        memory_ids: Vec<String>,
        summary: String,
    },
    /// Promote a memory to a higher tier.
    Promote {
        memory_id: String,
        from_tier: String,
        to_tier: String,
    },
    /// Demote a memory to a lower tier.
    Demote {
        memory_id: String,
        from_tier: String,
        to_tier: String,
    },
    /// Create a new memory (e.g., dream summary, pattern).
    Create { memory: crate::memory::Memory },
}

/// Review status for a dream action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewStatus {
    Pending,
    Approved,
    Rejected,
}

/// A single item in the dream review queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamReviewItem {
    /// Unique item ID.
    pub id: String,
    /// Dream run that produced this action.
    pub dream_id: String,
    /// Dream phase (light/deep/rem).
    pub phase: DreamPhase,
    /// Proposed action.
    pub action: DreamAction,
    /// Review status.
    pub status: ReviewStatus,
    /// When the item was created.
    pub created_at: SystemTime,
}

/// Human-in-the-loop review queue for dream actions.
///
/// When attached to a `DreamEngine`, proposed changes are enqueued
/// instead of applied immediately. The caller can review, approve, or
/// reject individual items, then apply approved actions in batch.
#[derive(Default)]
pub struct DreamReviewQueue {
    items: RwLock<Vec<DreamReviewItem>>,
}

/// Maximum number of completed (approved/rejected) items retained before
/// automatic cleanup.
const MAX_COMPLETED_REVIEW_ITEMS: usize = 1000;

impl DreamReviewQueue {
    /// Create a new empty review queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue a proposed dream action.
    pub async fn enqueue(&self, dream_id: &str, phase: DreamPhase, action: DreamAction) {
        let item = DreamReviewItem {
            id: uuid::Uuid::new_v4().to_string(),
            dream_id: dream_id.to_string(),
            phase,
            action,
            status: ReviewStatus::Pending,
            created_at: SystemTime::now(),
        };
        self.items.write().await.push(item);
    }

    /// Approve a review item by ID.
    pub async fn approve(&self, id: &str) -> bool {
        let mut items = self.items.write().await;
        if let Some(item) = items.iter_mut().find(|i| i.id == id) {
            item.status = ReviewStatus::Approved;
            true
        } else {
            false
        }
    }

    /// Reject a review item by ID.
    pub async fn reject(&self, id: &str) -> bool {
        let mut items = self.items.write().await;
        if let Some(item) = items.iter_mut().find(|i| i.id == id) {
            item.status = ReviewStatus::Rejected;
            true
        } else {
            false
        }
    }

    /// List all pending review items.
    pub async fn list_pending(&self) -> Vec<DreamReviewItem> {
        self.items
            .read()
            .await
            .iter()
            .filter(|i| i.status == ReviewStatus::Pending)
            .cloned()
            .collect()
    }

    /// Count pending items.
    pub async fn pending_count(&self) -> usize {
        self.items
            .read()
            .await
            .iter()
            .filter(|i| i.status == ReviewStatus::Pending)
            .count()
    }

    /// Apply all approved actions to the memory store and tier index.
    ///
    /// This executes the actual changes that were approved by the reviewer.
    /// Returns the number of actions that successfully mutated the store.
    pub async fn apply_approved(
        &self,
        store: &dyn MemoryStore,
        tier_index: &TierIndex,
    ) -> crate::Result<usize> {
        let mut applied = 0;
        let mut items = self.items.write().await;
        for i in 0..items.len() {
            if items[i].status != ReviewStatus::Approved {
                continue;
            }
            let action_applied = match &items[i].action {
                DreamAction::Delete { memory_id, .. } => {
                    match store.delete(&crate::memory::MemoryId::new(memory_id)).await {
                        Ok(_) => {
                            tier_index.remove(memory_id);
                            true
                        }
                        Err(e) => {
                            warn!("Dream apply: failed to delete memory {}: {e}", memory_id);
                            false
                        }
                    }
                }
                DreamAction::Merge { memory_ids, summary } => {
                    // Store summary FIRST so data is never lost on failure.
                    let mem = crate::memory::Memory::new("system", summary.clone(), "dream_merge")
                        .with_importance_score(0.7)
                        .with_source("dream_review");
                    let merge_applied = match store.store(mem).await {
                        Ok(summary_id) => {
                            // Summary stored — now delete source memories.
                            // A partial delete failure leaves the summary intact.
                            let mut delete_ok = true;
                            for id in memory_ids {
                                match store.delete(&crate::memory::MemoryId::new(id)).await {
                                    Ok(_) => {
                                        tier_index.remove(id);
                                    }
                                    Err(e) => {
                                        warn!(
                                            "Dream apply: failed to delete memory {} during \
                                             merge: {e}",
                                            id
                                        );
                                        delete_ok = false;
                                    }
                                }
                            }
                            if !delete_ok {
                                warn!(
                                    "Dream apply: merge partially complete — summary stored as {} \
                                     but some source memories could not be deleted",
                                    summary_id
                                );
                            }
                            tier_index.insert(
                                summary_id.to_string(),
                                crate::memory::tier::MemoryTier::LongTerm,
                            );
                            delete_ok
                        }
                        Err(e) => {
                            warn!("Dream apply: failed to store merge summary: {e}");
                            false
                        }
                    };
                    merge_applied
                }
                DreamAction::Promote { memory_id, to_tier, .. } => {
                    match crate::memory::tier::MemoryTier::from_label(to_tier) {
                        Ok(tier) => {
                            tier_index.update_tier(memory_id, tier);
                            true
                        }
                        Err(_) => false,
                    }
                }
                DreamAction::Demote { memory_id, to_tier, .. } => {
                    match crate::memory::tier::MemoryTier::from_label(to_tier) {
                        Ok(tier) => {
                            tier_index.update_tier(memory_id, tier);
                            true
                        }
                        Err(_) => false,
                    }
                }
                DreamAction::Create { memory } => match store.store(memory.clone()).await {
                    Ok(_) => true,
                    Err(e) => {
                        warn!("Dream apply: failed to create memory: {e}");
                        false
                    }
                },
            };
            // Only flip status after the action has been applied successfully,
            // so a failed action can be retried on the next call.
            if action_applied {
                items[i].status = ReviewStatus::Rejected; // reuse Rejected as "applied"
                applied += 1;
            }
        }
        // Trim completed items to prevent unbounded growth.
        if items.len() > MAX_COMPLETED_REVIEW_ITEMS {
            let completed_count = items
                .iter()
                .filter(|i| i.status != ReviewStatus::Pending)
                .count();
            if completed_count > MAX_COMPLETED_REVIEW_ITEMS / 2 {
                items.retain(|i| i.status == ReviewStatus::Pending);
            }
        }
        Ok(applied)
    }

    /// Persist the review queue to a JSON file.
    pub async fn persist_to(&self, path: impl AsRef<std::path::Path>) -> crate::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                crate::error::SyscityError::Storage {
                    context: format!("Failed to create review queue directory: {:?}", parent),
                    details: e.to_string(),
                }
            })?;
        }
        let items = self.items.read().await;
        let json = serde_json::to_string_pretty(&*items).map_err(|e| {
            crate::error::SyscityError::Storage {
                context: "Failed to serialize review queue".to_string(),
                details: e.to_string(),
            }
        })?;
        tokio::fs::write(path, json)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to write review queue".to_string(),
                details: e.to_string(),
            })?;
        Ok(())
    }

    /// Load the review queue from a JSON file.
    pub async fn load_from(path: impl AsRef<std::path::Path>) -> crate::Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::new());
        }
        let json = tokio::fs::read_to_string(path).await.map_err(|e| {
            crate::error::SyscityError::Storage {
                context: "Failed to read review queue".to_string(),
                details: e.to_string(),
            }
        })?;
        let items: Vec<DreamReviewItem> =
            serde_json::from_str(&json).map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to deserialize review queue".to_string(),
                details: e.to_string(),
            })?;
        Ok(Self { items: RwLock::new(items) })
    }

    /// Clear all items from the queue.
    pub async fn clear(&self) {
        self.items.write().await.clear();
    }
}
