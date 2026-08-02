//! MemoryManagerBuilder convenience builder.

use super::*;

/// Builder for MemoryManager (convenience).
#[derive(Default)]
pub struct MemoryManagerBuilder {
    config: MemoryManagerConfig,
    pub(super) pipeline: Option<EmbeddingPipelineHandle>,
    pub(super) vector_service: Option<Arc<VectorMemoryService>>,
    pub(super) session_search: Option<Arc<SessionSearch>>,
    qmd_executor: Option<Arc<QmdExecutor>>,
    multimodal_store: Option<Arc<MultimodalStore>>,
    effectiveness_tracker: Option<Arc<EffectivenessTracker>>,
    tier_index: Option<Arc<TierIndex>>,
}

impl MemoryManagerBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn config(mut self, config: MemoryManagerConfig) -> Self {
        self.config = config;
        self
    }

    pub fn pipeline(mut self, pipeline: EmbeddingPipelineHandle) -> Self {
        self.pipeline = Some(pipeline);
        self
    }

    /// Enable hybrid search by attaching a vector service.
    pub fn vector_service(mut self, svc: Arc<VectorMemoryService>) -> Self {
        self.vector_service = Some(svc);
        self
    }

    /// Enable hybrid search by attaching an FTS5 session search.
    pub fn session_search(mut self, ss: Arc<SessionSearch>) -> Self {
        self.session_search = Some(ss);
        self
    }

    /// Attach a QMD executor.
    pub fn qmd_executor(mut self, executor: Arc<QmdExecutor>) -> Self {
        self.qmd_executor = Some(executor);
        self
    }

    /// Attach a multimodal store.
    pub fn multimodal_store(mut self, store: Arc<MultimodalStore>) -> Self {
        self.multimodal_store = Some(store);
        self
    }

    /// Attach an effectiveness tracker.
    pub fn effectiveness_tracker(mut self, tracker: Arc<EffectivenessTracker>) -> Self {
        self.effectiveness_tracker = Some(tracker);
        self
    }

    /// Attach a tier index.
    pub fn tier_index(mut self, index: Arc<TierIndex>) -> Self {
        self.tier_index = Some(index);
        self
    }

    pub async fn build(self, database_url: impl AsRef<str>) -> crate::Result<MemoryManager> {
        let (store, chat_history): (Arc<dyn MemoryStore>, Arc<dyn ChatHistoryStore>);

        if self.config.enable_tiers {
            if let Some(ref workspace_dir) = self.config.workspace_dir {
                let mut tiered = TieredStore::new(workspace_dir.join("memory")).await?;
                if let Some(ref et) = self.effectiveness_tracker {
                    tiered = tiered.with_effectiveness_config(et.config().clone());
                }
                let short_term = tiered.short_term();
                store = Arc::new(tiered);
                chat_history = Arc::new(short_term);
            } else {
                let db = Arc::new(UnifiedStore::new(database_url.as_ref()).await?);
                store = db.clone();
                chat_history = db;
            }
        } else {
            let db = Arc::new(UnifiedStore::new(database_url.as_ref()).await?);
            store = db.clone();
            chat_history = db;
        }

        let mut mm = MemoryManager::new(store, chat_history, self.config);

        if let Some(pipeline) = self.pipeline {
            mm = mm.with_pipeline(pipeline);
        }
        if let Some(vs) = self.vector_service {
            mm = mm.with_vector_service(vs);
        }
        if let Some(ss) = self.session_search {
            mm = mm.with_session_search(ss);
        }
        if let Some(qmd) = self.qmd_executor {
            mm = mm.with_qmd_executor(qmd);
        }
        if let Some(ms) = self.multimodal_store {
            mm = mm.with_multimodal_store(ms);
        }
        if let Some(et) = self.effectiveness_tracker {
            mm = mm.with_effectiveness_tracker(et);
        }
        if let Some(ti) = self.tier_index {
            mm = mm.with_tier_index(ti);
        }

        Ok(mm)
    }
}
