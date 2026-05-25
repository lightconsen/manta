//! Memory Manager — unified orchestrator for Manta's memory system
//!
//! Wires together:
//! - UnifiedStore (SQLite + WAL + FTS5)
//! - EmbeddingPipeline (batched async embeddings)
//! - MemoryStore search with embedding support
//!
//! Provides three-tier memory access:
//! 1. Working — in-flight Context (handled by Agent)
//! 2. Episodic — session history via chat_messages table
//! 3. Semantic — extracted facts/prefs via memories table with embeddings

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::providers::{CompletionRequest, Message, Provider};

use super::{
    effectiveness::{EffectivenessConfig, EffectivenessTracker},
    events::{MemoryEventBuilder, MemoryEventLog},
    hybrid::{hybrid_search, HybridSearchConfig},
    multimodal::{MemoryMultimodalConfig, MultimodalStore},
    pipeline::EmbeddingPipelineHandle,
    qmd::{QmdExecutor, QmdScope},
    session_search::SessionSearch,
    tier::{TierIndex, TierSystemConfig},
    vector::VectorMemoryService,
    ChatHistoryStore, ChatMessage, Memory, MemoryId, MemoryQuery, MemoryStats, MemoryStore,
    TieredStore, UnifiedStore,
};

/// Configuration for the MemoryManager.
#[derive(Debug, Clone)]
pub struct MemoryManagerConfig {
    /// Maximum memories to inject into context per turn
    pub max_context_memories: usize,
    /// Whether to use the embedding pipeline (vs direct embedding)
    pub use_pipeline: bool,
    /// Config for hybrid search (vector + FTS5). Used when both
    /// `vector_service` and `session_search` are attached to the manager.
    pub hybrid_config: HybridSearchConfig,
    /// Workspace directory for multimodal storage and event logs.
    pub workspace_dir: Option<std::path::PathBuf>,
    /// Whether to enable effectiveness tracking.
    pub track_effectiveness: bool,
    /// Whether to enable tier management.
    pub enable_tiers: bool,
}

impl Default for MemoryManagerConfig {
    fn default() -> Self {
        Self {
            max_context_memories: 5,
            use_pipeline: true,
            hybrid_config: HybridSearchConfig::default(),
            workspace_dir: None,
            track_effectiveness: true,
            enable_tiers: true,
        }
    }
}

/// Maximum number of recent recalls to track per session before LRU eviction.
const MAX_RECENT_RECALLS_PER_SESSION: usize = 100;

/// Tracks a recent recall so it can be evaluated for a hit after the LLM responds.
#[derive(Debug, Clone)]
struct RecentRecall {
    recall_id: String,
    memory_content: String,
}

/// The MemoryManager orchestrates all memory operations.
pub struct MemoryManager {
    /// Primary memory store (may be tiered or unified).
    store: Arc<dyn MemoryStore>,
    /// Chat history store (always a DatabaseStore for persistence).
    chat_history: Arc<dyn ChatHistoryStore>,
    config: MemoryManagerConfig,
    /// Embedding pipeline handle (optional if pipeline not configured)
    pipeline: Option<EmbeddingPipelineHandle>,
    /// Vector memory service for semantic search (hybrid path)
    vector_service: Option<Arc<VectorMemoryService>>,
    /// FTS5 session search (hybrid path)
    session_search: Option<Arc<SessionSearch>>,
    /// In-memory cache of the last retrieved context (to avoid repeated DB hits)
    context_cache: RwLock<Option<ContextCache>>,
    /// Event log for memory operations.
    event_log: Option<MemoryEventLog>,
    /// Tier index for memory tier management.
    tier_index: Option<Arc<TierIndex>>,
    /// Effectiveness tracker for recall hit rates.
    effectiveness: Option<Arc<EffectivenessTracker>>,
    /// Multimodal file store.
    multimodal_store: Option<Arc<MultimodalStore>>,
    /// QMD executor for scope-based queries.
    qmd_executor: Option<Arc<QmdExecutor>>,
    /// Recent recalls per session, awaiting hit evaluation.
    /// session_key -> Vec<(recall_id, memory_content)>
    recent_recalls: RwLock<HashMap<String, Vec<RecentRecall>>>,
    /// Last time effectiveness adjustments were applied (rate limiting).
    last_adjustment: RwLock<Option<std::time::Instant>>,
    /// Optional LLM provider for session compaction and fact extraction.
    llm_provider: Option<Arc<dyn Provider>>,
}

impl std::fmt::Debug for MemoryManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryManager")
            .field("store", &"<dyn MemoryStore>")
            .field("chat_history", &"<dyn ChatHistoryStore>")
            .field("config", &self.config)
            .field("pipeline", &self.pipeline.is_some())
            .field("vector_service", &self.vector_service.is_some())
            .field("session_search", &self.session_search.is_some())
            .field("context_cache", &self.context_cache)
            .field("event_log", &self.event_log.is_some())
            .field("tier_index", &self.tier_index.is_some())
            .field("effectiveness", &self.effectiveness.is_some())
            .field("multimodal_store", &self.multimodal_store.is_some())
            .field("qmd_executor", &self.qmd_executor.is_some())
            .field("recent_recalls", &"<HashMap>")
            .field("llm_provider", &self.llm_provider.as_ref().map(|p| p.name()))
            .finish()
    }
}

/// Cached context to avoid repeated DB hits within a short window.
#[derive(Debug, Clone)]
struct ContextCache {
    user_id: String,
    conversation_id: String,
    memories: Vec<Memory>,
    multimodal_references: Vec<String>,
    cached_at: std::time::Instant,
}

impl ContextCache {
    /// TTL for the context cache
    const TTL_MS: u64 = 5000;

    fn is_valid(&self, user_id: &str, conversation_id: &str) -> bool {
        self.user_id == user_id
            && self.conversation_id == conversation_id
            && self.cached_at.elapsed().as_millis() < Self::TTL_MS as u128
    }
}

impl MemoryManager {
    /// Create a new MemoryManager with the given store and chat history.
    ///
    /// When using a unified store, pass the same `Arc` for both arguments.
    /// When using a tiered store, pass the tiered store as `store` and the
    /// short-term backend as `chat_history`.
    pub fn new(
        store: Arc<dyn MemoryStore>,
        chat_history: Arc<dyn ChatHistoryStore>,
        config: MemoryManagerConfig,
    ) -> Self {
        let event_log = config.workspace_dir.as_ref().map(MemoryEventLog::new);
        let tier_index = if config.enable_tiers {
            Some(Arc::new(TierIndex::new()))
        } else {
            None
        };
        let effectiveness = if config.track_effectiveness {
            Some(Arc::new(EffectivenessTracker::new(EffectivenessConfig::default())))
        } else {
            None
        };
        let multimodal_store = config
            .workspace_dir
            .as_ref()
            .map(|d| Arc::new(MultimodalStore::new(d, MemoryMultimodalConfig::default())));
        let qmd_executor = config
            .workspace_dir
            .as_ref()
            .map(|d| Arc::new(QmdExecutor::new(d)));

        Self {
            store,
            chat_history,
            config,
            pipeline: None,
            vector_service: None,
            session_search: None,
            context_cache: RwLock::new(None),
            event_log,
            tier_index,
            effectiveness,
            multimodal_store,
            qmd_executor,
            recent_recalls: RwLock::new(HashMap::new()),
            last_adjustment: RwLock::new(None),
            llm_provider: None,
        }
    }

    /// Attach an embedding pipeline for async embedding generation.
    pub fn with_pipeline(mut self, pipeline: EmbeddingPipelineHandle) -> Self {
        self.pipeline = Some(pipeline);
        self
    }

    /// Attach a vector memory service to enable the hybrid search path.
    ///
    /// Hybrid search is active only when *both* `vector_service` and
    /// `session_search` are set.  When active, `retrieve()` calls
    /// [`hybrid_search`] instead of the plain `DatabaseStore` query.
    pub fn with_vector_service(mut self, svc: Arc<VectorMemoryService>) -> Self {
        self.vector_service = Some(svc);
        self
    }

    /// Attach a session search (FTS5) backend to enable the hybrid search path.
    pub fn with_session_search(mut self, ss: Arc<SessionSearch>) -> Self {
        self.session_search = Some(ss);
        self
    }

    /// Attach a QMD executor for scope-based queries.
    pub fn with_qmd_executor(mut self, executor: Arc<QmdExecutor>) -> Self {
        self.qmd_executor = Some(executor);
        self
    }

    /// Attach a multimodal store.
    pub fn with_multimodal_store(mut self, store: Arc<MultimodalStore>) -> Self {
        self.multimodal_store = Some(store);
        self
    }

    /// Attach an effectiveness tracker.
    pub fn with_effectiveness_tracker(mut self, tracker: Arc<EffectivenessTracker>) -> Self {
        self.effectiveness = Some(tracker);
        self
    }

    /// Attach a tier index.
    pub fn with_tier_index(mut self, index: Arc<TierIndex>) -> Self {
        self.tier_index = Some(index);
        self
    }

    /// Attach an LLM provider for session compaction and fact extraction.
    pub fn with_llm_provider(mut self, provider: Arc<dyn Provider>) -> Self {
        self.llm_provider = Some(provider);
        self
    }

    /// Get the event log (if configured).
    pub fn event_log(&self) -> Option<&MemoryEventLog> {
        self.event_log.as_ref()
    }

    /// Get the tier index (if configured).
    pub fn tier_index(&self) -> Option<Arc<TierIndex>> {
        self.tier_index.clone()
    }

    /// Get the effectiveness tracker (if configured).
    pub fn effectiveness_tracker(&self) -> Option<Arc<EffectivenessTracker>> {
        self.effectiveness.clone()
    }

    /// Get the multimodal store (if configured).
    pub fn multimodal_store(&self) -> Option<Arc<MultimodalStore>> {
        self.multimodal_store.clone()
    }

    /// Get the QMD executor (if configured).
    pub fn qmd_executor(&self) -> Option<Arc<QmdExecutor>> {
        self.qmd_executor.clone()
    }

    // =============================================================================
    // Core API: observe, retrieve, session_context
    // =============================================================================

    /// Observe a fact and store it with an embedding.
    ///
    /// This is the primary write path for semantic memories.  The embedding
    /// is generated via the pipeline if configured, otherwise skipped (to be
    /// filled later by background job).
    pub async fn observe(
        &self,
        user_id: impl Into<String>,
        content: impl Into<String>,
        memory_type: impl Into<String>,
        importance: f32,
    ) -> crate::Result<MemoryId> {
        let user_id = user_id.into();
        let content = content.into();
        let memory_type = memory_type.into();

        debug!(
            "Observing memory: user={} type={} importance={}",
            user_id, memory_type, importance
        );

        // Generate embedding via pipeline if available
        let embedding = if let Some(ref pipeline) = self.pipeline {
            match pipeline.embed(&content).await {
                Ok(emb) => Some(emb),
                Err(e) => {
                    warn!("Embedding failed, storing without embedding: {}", e);
                    None
                }
            }
        } else {
            None
        };

        let memory = Memory::new(&user_id, content, memory_type)
            .with_importance_score(importance)
            .with_source("agent");

        let memory = if let Some(emb) = embedding {
            memory.with_embedding(emb)
        } else {
            memory
        };

        let id = self.store.store(memory.clone()).await?;

        // Register in tier index if enabled
        if let Some(ref tier_index) = self.tier_index {
            let entry_tier = super::tier::TierEvaluator::new(TierSystemConfig::default())
                .entry_tier(importance, 0);
            tier_index.insert(id.to_string(), entry_tier);
        }

        info!("Memory observed: {}", id);
        Ok(id)
    }

    /// Retrieve relevant memories for a query.
    ///
    /// **Hybrid path** (when both `vector_service` and `session_search` are
    /// attached): runs [`hybrid_search`] concurrently over the vector store and
    /// the FTS5 session index, merges with weighted scoring, applies MMR
    /// re-ranking, and converts results to [`Memory`] values.
    ///
    /// **Fallback path** (when either backend is absent):
    /// 1. Embed the query via pipeline (if available)
    /// 2. Search `DatabaseStore` with embedding similarity or LIKE text search
    pub async fn retrieve(
        &self,
        user_id: impl AsRef<str>,
        conversation_id: Option<&str>,
        query: impl Into<String>,
        limit: Option<usize>,
    ) -> crate::Result<Vec<Memory>> {
        let user_id = user_id.as_ref();
        let query_text = query.into();
        let limit = limit.unwrap_or(self.config.max_context_memories);

        debug!(
            "Retrieving memories: user={} query_len={} hybrid={}",
            user_id,
            query_text.len(),
            self.vector_service.is_some() && self.session_search.is_some(),
        );

        let mut memories: Vec<Memory>;

        // ── Hybrid path ───────────────────────────────────────────────────────
        if let (Some(ref vs), Some(ref ss)) = (&self.vector_service, &self.session_search) {
            let mut cfg = self.config.hybrid_config.clone();
            cfg.max_results = limit;

            let hybrid_results = hybrid_search(&query_text, vs, ss, &cfg).await;

            memories = hybrid_results
                .into_iter()
                .map(|r| {
                    Memory::new(user_id, r.content, "hybrid")
                        .with_importance_score(r.score)
                        .with_source(&r.source)
                        .with_metadata(serde_json::json!({
                            "citation": r.citation,
                            "hybrid_source": r.source,
                        }))
                })
                .collect();
        } else {
            // ── Fallback: DatabaseStore path ──────────────────────────────────────

            // Embed the query if pipeline available
            let query_embedding = if let Some(ref pipeline) = self.pipeline {
                match pipeline.embed(&query_text).await {
                    Ok(emb) => Some(emb),
                    Err(e) => {
                        warn!("Query embedding failed: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            // Build and execute query
            let mut mq = MemoryQuery::new().for_user(user_id).limit(limit);

            if let Some(conv_id) = conversation_id {
                mq = mq.for_conversation(conv_id);
            }

            if let Some(ref embedding) = query_embedding {
                mq = mq.with_embedding(embedding.clone());
            } else {
                mq = mq.with_content(&query_text);
            }

            memories = self.store.search(mq).await?;
        }

        // ── QMD search path ───────────────────────────────────────────────────
        if let Some(ref qmd) = self.qmd_executor {
            let scope = QmdScope::default().with_key_prefix(format!("{}:", user_id));
            match qmd.query(&query_text, Some(&scope)).await {
                Ok(qmd_results) => {
                    let existing: std::collections::HashSet<String> =
                        memories.iter().map(|m| m.content.clone()).collect();
                    for qr in qmd_results.into_iter().take(limit) {
                        if let Some(body) = qr.body.or(qr.snippet) {
                            if existing.contains(&body) {
                                continue;
                            }
                            let score = qr.score.unwrap_or(0.5) as f32;
                            memories.push(
                                Memory::new(user_id, body, "qmd")
                                    .with_importance_score(score)
                                    .with_source("qmd"),
                            );
                        }
                    }
                    memories.sort_by(|a, b| {
                        b.importance_score
                            .partial_cmp(&a.importance_score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    memories.truncate(limit);
                }
                Err(e) => {
                    warn!("QMD query failed: {}", e);
                }
            }
        }

        // Update cache
        if let Some(conv_id) = conversation_id {
            let cache = ContextCache {
                user_id: user_id.to_string(),
                conversation_id: conv_id.to_string(),
                memories: memories.clone(),
                multimodal_references: vec![],
                cached_at: std::time::Instant::now(),
            };
            *self.context_cache.write().await = Some(cache);
        }

        // Log recall events and track effectiveness
        let session_key = conversation_id
            .map(|c| format!("{}:{}", user_id, c))
            .unwrap_or_else(|| user_id.to_string());

        for (rank, mem) in memories.iter().enumerate() {
            // Record access in tier index
            if let Some(ref tier_index) = self.tier_index {
                tier_index.record_access(&mem.id.to_string());
            }

            // Track effectiveness
            if let Some(ref effectiveness) = self.effectiveness {
                let recall_id = format!("recall-{}", uuid::Uuid::new_v4());
                effectiveness
                    .record_recall(
                        recall_id.clone(),
                        &mem.id.to_string(),
                        &session_key,
                        &mem.memory_type,
                        mem.importance_score,
                        rank,
                    )
                    .await;

                // Store recall for later hit evaluation
                let mut recent_guard = self.recent_recalls.write().await;
                let recalls = recent_guard.entry(session_key.clone()).or_default();
                recalls.push(RecentRecall {
                    recall_id,
                    memory_content: mem.content.clone(),
                });
                // Enforce per-session bound to prevent unbounded growth
                if recalls.len() > MAX_RECENT_RECALLS_PER_SESSION {
                    recalls.remove(0);
                }
            }

            // Log recall event
            if let Some(ref event_log) = self.event_log {
                let event = MemoryEventBuilder::new().recall(
                    &session_key,
                    format!("recall-{}", uuid::Uuid::new_v4()),
                    &mem.memory_type,
                    mem.content.chars().take(100).collect::<String>(),
                );
                if let Err(e) = event_log.append(&event).await {
                    warn!("Failed to append recall event: {}", e);
                }
            }
        }

        Ok(memories)
    }

    /// Retrieve session context for a conversation.
    ///
    /// Returns:
    /// 1. Recent chat messages (episodic)
    /// 2. Relevant semantic memories (via retrieve)
    pub async fn session_context(
        &self,
        user_id: impl AsRef<str>,
        conversation_id: impl AsRef<str>,
        query: Option<impl Into<String>>,
    ) -> crate::Result<SessionContext> {
        let user_id = user_id.as_ref();
        let conversation_id = conversation_id.as_ref();

        // Check cache first
        {
            let guard = self.context_cache.read().await;
            if let Some(ref cache) = *guard {
                if cache.is_valid(user_id, conversation_id) {
                    return Ok(SessionContext {
                        messages: self
                            .chat_history
                            .get_conversation_history(conversation_id, 50)
                            .await?,
                        memories: cache.memories.clone(),
                        multimodal_references: cache.multimodal_references.clone(),
                    });
                }
            }
        }

        // Episodic: recent messages
        let messages = self
            .chat_history
            .get_conversation_history(conversation_id, 50)
            .await?;

        // Semantic: relevant memories
        let memories = if let Some(q) = query {
            self.retrieve(user_id, Some(conversation_id), q, Some(self.config.max_context_memories))
                .await?
        } else {
            // No query, fetch recent high-importance memories
            let mq = MemoryQuery::new()
                .for_user(user_id)
                .for_conversation(conversation_id)
                .limit(self.config.max_context_memories);
            self.store.search(mq).await?
        };

        // Multimodal: scan for image/audio files in workspace
        let mut multimodal_references = Vec::new();
        if let Some(ref mm_store) = self.multimodal_store {
            use super::multimodal::MemoryMultimodalModality;
            for modality in [
                MemoryMultimodalModality::Image,
                MemoryMultimodalModality::Audio,
            ] {
                let files = mm_store.scan_modality(modality).await;
                for entry in files.into_iter().take(5) {
                    let label = entry
                        .label
                        .unwrap_or_else(|| format!("{} file: {}", modality, entry.filename));
                    multimodal_references.push(format!("[{}]", label));
                }
            }
        }

        // Update cache
        let ctx = SessionContext {
            messages,
            memories,
            multimodal_references,
        };
        {
            let cache = ContextCache {
                user_id: user_id.to_string(),
                conversation_id: conversation_id.to_string(),
                memories: ctx.memories.clone(),
                multimodal_references: ctx.multimodal_references.clone(),
                cached_at: std::time::Instant::now(),
            };
            *self.context_cache.write().await = Some(cache);
        }

        Ok(ctx)
    }

    /// Remember a user message (store in chat history).
    pub async fn remember_message(
        &self,
        user_id: impl Into<String>,
        conversation_id: impl Into<String>,
        role: impl Into<String>,
        content: impl Into<String>,
    ) -> crate::Result<()> {
        let msg = ChatMessage::new(conversation_id, user_id, role, content);
        self.chat_history.store_message(msg).await
    }

    /// Get the last conversation ID for a user.
    pub async fn last_conversation(
        &self,
        user_id: impl AsRef<str>,
    ) -> crate::Result<Option<String>> {
        self.chat_history
            .get_last_conversation(user_id.as_ref())
            .await
    }

    /// Forget (delete) a memory by ID.
    pub async fn forget(&self, id: &MemoryId) -> crate::Result<bool> {
        self.store.delete(id).await
    }

    /// Compact a session: extract key facts from old messages into semantic memories.
    ///
    /// This is called when a session is closed or exceeds thresholds
    /// (>50 turns or >7 days old).
    ///
    /// When an LLM provider is attached, uses the model to extract facts,
    /// preferences, decisions, and important context.  Falls back to naive
    /// sampling when no provider is configured.
    pub async fn compact_session(
        &self,
        conversation_id: impl AsRef<str>,
        model: Option<&str>,
    ) -> crate::Result<Vec<MemoryId>> {
        let conversation_id = conversation_id.as_ref();
        info!("Compacting session: {}", conversation_id);

        // Get full session history
        let messages = self
            .chat_history
            .get_conversation_history(conversation_id, 1000)
            .await?;

        if messages.len() < 10 {
            debug!("Session too short to compact: {} messages", messages.len());
            return Ok(vec![]);
        }

        let user_id = messages
            .iter()
            .find(|m| !m.user_id.is_empty())
            .map(|m| m.user_id.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let stored_ids = if let Some(ref provider) = self.llm_provider {
            self.compact_with_llm(provider, model, &user_id, conversation_id, &messages)
                .await?
        } else {
            self.compact_naive(&user_id, &messages).await?
        };

        // Mark session as compacted
        if !stored_ids.is_empty() {
            let marker = Memory::new(
                &user_id,
                format!("Session compacted: {}", conversation_id),
                "compaction",
            )
            .with_conversation(conversation_id)
            .with_metadata(serde_json::json!({
                "compacted_at": chrono::Utc::now().to_rfc3339(),
                "message_count": messages.len(),
                "extracted_memories": stored_ids.len(),
            }))
            .with_source("compaction");

            self.store.store(marker).await?;
        }

        let session_key = format!("{}:{}", user_id, conversation_id);

        // Log compact event
        if let Some(ref event_log) = self.event_log {
            let event = MemoryEventBuilder::new().compact(
                &session_key,
                format!("compact-{}", uuid::Uuid::new_v4()),
                messages.len() as u32,
                stored_ids.len() as u32,
            );
            if let Err(e) = event_log.append(&event).await {
                warn!("Failed to append compact event: {}", e);
            }
        }

        info!("Session {} compacted: {} facts extracted", conversation_id, stored_ids.len());
        Ok(stored_ids)
    }

    /// Compact a session using an LLM to extract facts, preferences, decisions,
    /// and important context from the conversation history.
    async fn compact_with_llm(
        &self,
        provider: &Arc<dyn Provider>,
        model: Option<&str>,
        user_id: &str,
        conversation_id: &str,
        messages: &[ChatMessage],
    ) -> crate::Result<Vec<MemoryId>> {
        let transcript: String = messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n\n");

        let prompt = format!(
            "You are an expert memory extraction assistant. \
            Analyze the following conversation and extract key facts, preferences, decisions, \
            and important context that should be remembered for future interactions.\n\n\
            Return your findings as a JSON array of objects, each with:\n\
            - \"content\": the fact/preference/decision as a concise statement\n\
            - \"type\": one of \"fact\", \"preference\", \"decision\", \"context\"\n\
            - \"importance\": a score from 0.0 to 1.0\n\n\
            Only extract information that is clearly stated or strongly implied. \
            Do not invent information. Return ONLY the JSON array, no other text.\n\n\
            Conversation:\n{transcript}"
        );

        let request = CompletionRequest {
            messages: vec![
                Message::system(
                    "You are a helpful assistant that extracts and structures \
                    information from conversations. Return only valid JSON.",
                ),
                Message::user(prompt),
            ],
            model: model.map(String::from),
            temperature: Some(0.3),
            max_tokens: Some(4096),
            stream: false,
            ..Default::default()
        };

        let response = match provider.complete(request).await {
            Ok(resp) => resp,
            Err(e) => {
                warn!("LLM compaction failed, falling back to naive extraction: {}", e);
                return self.compact_naive(user_id, messages).await;
            }
        };

        let extracted: Vec<serde_json::Value> =
            match serde_json::from_str(&response.message.content) {
                Ok(vals) => vals,
                Err(e) => {
                    warn!("Failed to parse LLM extraction JSON, falling back: {}", e);
                    return self.compact_naive(user_id, messages).await;
                }
            };

        let mut stored_ids = Vec::new();
        for item in extracted {
            let content = item.get("content").and_then(|v| v.as_str());
            let memory_type = item
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("semantic");
            let importance = item
                .get("importance")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.5) as f32;

            if let Some(content) = content {
                if content.len() >= 5 {
                    if let Ok(id) = self
                        .observe(user_id, content, memory_type, importance)
                        .await
                    {
                        stored_ids.push(id);
                    }
                }
            }
        }

        debug!(
            "LLM extraction yielded {} memories from session {}",
            stored_ids.len(),
            conversation_id
        );
        Ok(stored_ids)
    }

    /// Fallback naive compaction: sample every 5th user message.
    async fn compact_naive(
        &self,
        user_id: &str,
        messages: &[ChatMessage],
    ) -> crate::Result<Vec<MemoryId>> {
        let mut stored_ids = vec![];

        for (i, msg) in messages.iter().enumerate() {
            if msg.role != "user" {
                continue;
            }
            if i % 5 != 0 {
                continue;
            }
            let fact = msg.content.clone();
            if fact.len() < 20 {
                continue;
            }

            let id = self.observe(user_id, fact, "semantic", 0.6).await?;

            stored_ids.push(id);
        }

        Ok(stored_ids)
    }

    /// Get the primary memory store.
    pub fn store(&self) -> Arc<dyn MemoryStore> {
        Arc::clone(&self.store)
    }

    /// Get the chat history store.
    pub fn chat_history(&self) -> Arc<dyn ChatHistoryStore> {
        Arc::clone(&self.chat_history)
    }

    /// Get memory statistics.
    pub async fn stats(&self) -> crate::Result<MemoryStats> {
        self.store.stats().await
    }

    /// Evaluate whether recently-recalled memories were "hit" by the LLM response.
    ///
    /// For each recent recall in `session_key`, checks if `response_text` contains
    /// a significant substring of the recalled memory content. If so, marks it as a hit
    /// in the effectiveness tracker.
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

    /// Evaluate recalled memories for effectiveness and adjust importance scores.
    ///
    /// Closes the feedback loop: uses the effectiveness tracker to evaluate
    /// memories that have been recalled recently, and adjusts their importance
    /// scores based on hit rates.
    ///
    /// Rate-limited: skips if adjustments were applied within the last 5 minutes.
    pub async fn apply_effectiveness_adjustments(&self) {
        let effectiveness = match &self.effectiveness {
            Some(e) => e.clone(),
            None => return,
        };

        // Rate limit: skip if adjustments were applied within the last 5 minutes
        {
            let guard = self.last_adjustment.read().await;
            if let Some(last) = *guard {
                if last.elapsed().as_secs() < 300 {
                    return;
                }
            }
        }

        // Collect memory IDs that have been tracked by effectiveness
        let Some(memory_ids) = self.collect_tracked_memory_ids().await else {
            return;
        };

        if memory_ids.is_empty() {
            return;
        }

        let mut adjusted = 0usize;

        for memory_id in memory_ids {
            // Get current memory to read importance score
            let Ok(Some(memory)) = self
                .store
                .get(&crate::memory::MemoryId::new(&memory_id))
                .await
            else {
                continue;
            };

            let action = effectiveness
                .evaluate(&memory_id, memory.importance_score)
                .await;
            if action == crate::memory::effectiveness::EffectivenessAction::NoOp {
                continue;
            }

            let old_score = memory.importance_score;
            let new_score = effectiveness.apply_action(action, old_score);
            if (new_score - old_score).abs() < 0.001 {
                continue;
            }

            // Update the memory with new importance score
            let mut updated = memory;
            updated.importance_score = new_score;
            if let Err(e) = self.store.update(updated).await {
                warn!("Failed to update memory effectiveness for {}: {}", memory_id, e);
                continue;
            }

            info!(
                "Effectiveness adjustment: memory {} importance {:.3} -> {:.3}",
                memory_id, old_score, new_score
            );
            adjusted += 1;
        }

        if adjusted > 0 {
            info!("Applied {} effectiveness adjustments", adjusted);
        }

        // Update last adjustment time
        *self.last_adjustment.write().await = Some(std::time::Instant::now());
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

/// Session context returned by `session_context()`.
#[derive(Debug, Clone)]
pub struct SessionContext {
    /// Recent chat messages (episodic memory)
    pub messages: Vec<ChatMessage>,
    /// Relevant semantic memories
    pub memories: Vec<Memory>,
    /// Multimodal file references (e.g. "[Image file: photo.png]")
    pub multimodal_references: Vec<String>,
}

impl SessionContext {
    /// Format the context as a system message injection.
    ///
    /// This produces the string that gets injected into the agent's
    /// context window before the conversation.
    pub fn format_for_injection(&self) -> String {
        let mut parts = vec![];

        // Multimodal references
        if !self.multimodal_references.is_empty() {
            parts.push(format!("## Attached Files\n{}", self.multimodal_references.join("\n")));
        }

        // Semantic memories
        if !self.memories.is_empty() {
            let mem_lines: Vec<String> = self
                .memories
                .iter()
                .map(|m| format!("- [{}] {}", m.memory_type, m.content))
                .collect();
            parts.push(format!("## Relevant Context\n{}", mem_lines.join("\n")));
        }

        // Episodic context (recent conversation summary if available)
        if self.messages.len() > 10 {
            let recent: Vec<String> = self
                .messages
                .iter()
                .rev()
                .take(5)
                .rev()
                .map(|m| format!("{}: {}", m.role, m.content.chars().take(100).collect::<String>()))
                .collect();
            parts.push(format!("## Recent Messages\n{}", recent.join("\n")));
        }

        parts.join("\n\n")
    }
}

/// Builder for MemoryManager (convenience).
#[derive(Default)]
pub struct MemoryManagerBuilder {
    config: MemoryManagerConfig,
    pipeline: Option<EmbeddingPipelineHandle>,
    vector_service: Option<Arc<VectorMemoryService>>,
    session_search: Option<Arc<SessionSearch>>,
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
                let tiered = TieredStore::new(workspace_dir.join("memory")).await?;
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_manager_observe_and_retrieve() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let mm = MemoryManager::new(store.clone(), store, MemoryManagerConfig::default());

        // Store some memories
        let id1 = mm
            .observe("user1", "I love sushi", "preference", 0.9)
            .await
            .unwrap();
        let id2 = mm
            .observe("user1", "I work at Google", "fact", 0.7)
            .await
            .unwrap();

        // Verify stored
        assert!(!id1.0.is_empty());
        assert!(!id2.0.is_empty());

        // Retrieve (no embedding, so falls back to text search)
        // Use a simpler query that will match with LIKE '%sushi%'
        let results = mm
            .retrieve("user1", None::<&str>, "sushi", Some(5))
            .await
            .unwrap();

        // Should find the sushi memory
        assert!(
            results.iter().any(|m| m.content.contains("sushi")),
            "Expected to find sushi memory"
        );
    }

    #[tokio::test]
    async fn test_memory_manager_session_context() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let mm = MemoryManager::new(store.clone(), store, MemoryManagerConfig::default());

        // Add messages
        mm.remember_message("user1", "conv1", "user", "Hello!")
            .await
            .unwrap();
        mm.remember_message("user1", "conv1", "assistant", "Hi there!")
            .await
            .unwrap();

        // Add a memory
        mm.observe("user1", "User likes pizza", "preference", 0.8)
            .await
            .unwrap();

        // Get context
        let ctx = mm
            .session_context("user1", "conv1", Some::<&str>("food"))
            .await
            .unwrap();

        assert_eq!(ctx.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_session_context_formatting() {
        let ctx = SessionContext {
            messages: vec![],
            memories: vec![
                Memory::new("u1", "Likes coffee", "preference"),
                Memory::new("u1", "Works remotely", "fact"),
            ],
            multimodal_references: vec![],
        };

        let formatted = ctx.format_for_injection();
        assert!(formatted.contains("Relevant Context"));
        assert!(formatted.contains("Likes coffee"));
    }

    #[test]
    fn test_memory_manager_config_default() {
        let config = MemoryManagerConfig::default();
        assert_eq!(config.max_context_memories, 5);
        assert!(config.use_pipeline);
    }

    #[test]
    fn test_context_cache_valid() {
        let cache = ContextCache {
            user_id: "u1".to_string(),
            conversation_id: "c1".to_string(),
            memories: vec![],
            multimodal_references: vec![],
            cached_at: std::time::Instant::now(),
        };
        assert!(cache.is_valid("u1", "c1"));
    }

    #[test]
    fn test_context_cache_invalid_user() {
        let cache = ContextCache {
            user_id: "u1".to_string(),
            conversation_id: "c1".to_string(),
            memories: vec![],
            multimodal_references: vec![],
            cached_at: std::time::Instant::now(),
        };
        assert!(!cache.is_valid("u2", "c1"));
    }

    #[test]
    fn test_context_cache_invalid_conversation() {
        let cache = ContextCache {
            user_id: "u1".to_string(),
            conversation_id: "c1".to_string(),
            memories: vec![],
            multimodal_references: vec![],
            cached_at: std::time::Instant::now(),
        };
        assert!(!cache.is_valid("u1", "c2"));
    }

    #[test]
    fn test_session_context_formatting_empty() {
        let ctx = SessionContext {
            messages: vec![],
            memories: vec![],
            multimodal_references: vec![],
        };
        let formatted = ctx.format_for_injection();
        assert!(!formatted.contains("Relevant Context"));
        assert!(!formatted.contains("Recent Messages"));
    }

    #[test]
    fn test_session_context_formatting_with_many_messages() {
        let mut messages = vec![];
        for i in 0..12 {
            messages.push(ChatMessage::new(
                "c1",
                "u1",
                if i % 2 == 0 { "user" } else { "assistant" },
                format!("msg {}", i),
            ));
        }
        let ctx = SessionContext {
            messages,
            memories: vec![Memory::new("u1", "Likes tea", "preference")],
            multimodal_references: vec![],
        };
        let formatted = ctx.format_for_injection();
        assert!(formatted.contains("Relevant Context"));
        assert!(formatted.contains("Recent Messages"));
    }

    #[test]
    fn test_memory_manager_builder_default() {
        let builder = MemoryManagerBuilder::default();
        assert!(builder.pipeline.is_none());
        assert!(builder.vector_service.is_none());
        assert!(builder.session_search.is_none());
    }

    #[tokio::test]
    async fn test_memory_manager_debug() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let mm = MemoryManager::new(store.clone(), store, MemoryManagerConfig::default());
        let debug = format!("{:?}", mm);
        assert!(debug.contains("MemoryManager"));
    }

    #[tokio::test]
    async fn test_memory_manager_remember_message_and_last_conversation() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let mm = MemoryManager::new(store.clone(), store, MemoryManagerConfig::default());

        mm.remember_message("u1", "conv-a", "user", "Hello")
            .await
            .unwrap();
        mm.remember_message("u1", "conv-a", "assistant", "Hi")
            .await
            .unwrap();

        let last = mm.last_conversation("u1").await.unwrap();
        assert_eq!(last, Some("conv-a".to_string()));
    }

    #[tokio::test]
    async fn test_memory_manager_forget() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let mm = MemoryManager::new(store.clone(), store, MemoryManagerConfig::default());

        let id = mm
            .observe("u1", "forgettable content", "test", 0.5)
            .await
            .unwrap();
        let deleted = mm.forget(&id).await.unwrap();
        assert!(deleted);

        // Forgetting again should return false
        let deleted_again = mm.forget(&id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_memory_manager_compact_session_short() {
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let mm = MemoryManager::new(store.clone(), store, MemoryManagerConfig::default());

        // Only 3 messages, less than threshold of 10
        for i in 0..3 {
            mm.remember_message("u1", "short-conv", "user", format!("msg {}", i))
                .await
                .unwrap();
        }

        let ids = mm.compact_session("short-conv", None).await.unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn test_evaluate_response_hits_marks_hit() {
        use crate::memory::effectiveness::{EffectivenessConfig, EffectivenessTracker};

        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let tracker = Arc::new(EffectivenessTracker::new(EffectivenessConfig::default()));
        let mm = MemoryManager::new(store.clone(), store, MemoryManagerConfig::default())
            .with_effectiveness_tracker(tracker.clone());

        let session_key = "u1:conv1";

        // Manually inject a recent recall
        {
            let mut guard = mm.recent_recalls.write().await;
            guard.insert(
                session_key.to_string(),
                vec![RecentRecall {
                    recall_id: "recall-001".to_string(),
                    memory_content: "The user loves hiking in the mountains".to_string(),
                }],
            );
        }

        // Record the recall in the tracker so mark_hit can find it
        tracker
            .record_recall("recall-001", "m1", session_key, "preference", 0.8, 0)
            .await;

        // Response contains the memory content
        mm.evaluate_response_hits(
            session_key,
            "I remember that the user loves hiking in the mountains!",
        )
        .await;

        let stats = tracker.memory_stats("m1").await.unwrap();
        assert_eq!(stats.total_recalls, 1);
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.hit_rate, 1.0);

        // recent_recalls should be cleared for this session
        let guard = mm.recent_recalls.read().await;
        assert!(!guard.contains_key(session_key));
    }

    #[tokio::test]
    async fn test_evaluate_response_hits_no_hit() {
        use crate::memory::effectiveness::{EffectivenessConfig, EffectivenessTracker};

        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let tracker = Arc::new(EffectivenessTracker::new(EffectivenessConfig::default()));
        let mm = MemoryManager::new(store.clone(), store, MemoryManagerConfig::default())
            .with_effectiveness_tracker(tracker.clone());

        let session_key = "u1:conv2";

        {
            let mut guard = mm.recent_recalls.write().await;
            guard.insert(
                session_key.to_string(),
                vec![RecentRecall {
                    recall_id: "recall-002".to_string(),
                    memory_content: "The user loves hiking in the mountains".to_string(),
                }],
            );
        }

        tracker
            .record_recall("recall-002", "m2", session_key, "preference", 0.8, 0)
            .await;

        // Response does NOT contain the memory content
        mm.evaluate_response_hits(session_key, "That sounds interesting, tell me more.")
            .await;

        let stats = tracker.memory_stats("m2").await.unwrap();
        assert_eq!(stats.total_recalls, 1);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.hit_rate, 0.0);
    }

    #[tokio::test]
    async fn test_evaluate_response_hits_full_closed_loop() {
        use crate::memory::effectiveness::{
            EffectivenessAction, EffectivenessConfig, EffectivenessTracker,
        };

        let config = EffectivenessConfig {
            auto_adjust: true,
            promotion_threshold: 0.7,
            demotion_threshold: 0.2,
            min_recalls_for_adjustment: 3,
            importance_boost: 0.1,
            importance_penalty: 0.1,
            max_importance: 1.0,
            min_importance: 0.0,
        };
        let store = Arc::new(UnifiedStore::new_in_memory().await.unwrap());
        let tracker = Arc::new(EffectivenessTracker::new(config));
        let mm = MemoryManager::new(store.clone(), store, MemoryManagerConfig::default())
            .with_effectiveness_tracker(tracker.clone());

        let session_key = "u1:conv3";

        // Simulate 3 recalls, all hit
        for i in 0..3 {
            let recall_id = format!("recall-high-{}", i);
            {
                let mut guard = mm.recent_recalls.write().await;
                guard
                    .entry(session_key.to_string())
                    .or_default()
                    .push(RecentRecall {
                        recall_id: recall_id.clone(),
                        memory_content: "User prefers dark mode".to_string(),
                    });
            }
            tracker
                .record_recall(&recall_id, "m_high", session_key, "preference", 0.6, 0)
                .await;
        }

        // Response contains the memory content
        mm.evaluate_response_hits(
            session_key,
            "I know that User prefers dark mode, so I'll set that up.",
        )
        .await;

        let stats = tracker.memory_stats("m_high").await.unwrap();
        assert_eq!(stats.total_recalls, 3);
        assert_eq!(stats.hits, 3);
        assert_eq!(stats.hit_rate, 1.0);

        let action = tracker.evaluate("m_high", 0.6).await;
        assert_eq!(action, EffectivenessAction::Boost);

        let new_importance = tracker.apply_action(action, 0.6);
        assert!((new_importance - 0.7).abs() < 0.001);
    }
}
