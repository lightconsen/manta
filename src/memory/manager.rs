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

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::{
    hybrid::{hybrid_search, HybridSearchConfig},
    pipeline::EmbeddingPipelineHandle,
    session_search::SessionSearch,
    vector::VectorMemoryService,
    ChatHistoryStore, ChatMessage, Memory, MemoryId, MemoryQuery, MemoryStore, MemoryStats,
    UnifiedStore,
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
}

impl Default for MemoryManagerConfig {
    fn default() -> Self {
        Self {
            max_context_memories: 5,
            use_pipeline: true,
            hybrid_config: HybridSearchConfig::default(),
        }
    }
}

/// The MemoryManager orchestrates all memory operations.
pub struct MemoryManager {
    store: Arc<UnifiedStore>,
    config: MemoryManagerConfig,
    /// Embedding pipeline handle (optional if pipeline not configured)
    pipeline: Option<EmbeddingPipelineHandle>,
    /// Vector memory service for semantic search (hybrid path)
    vector_service: Option<Arc<VectorMemoryService>>,
    /// FTS5 session search (hybrid path)
    session_search: Option<Arc<SessionSearch>>,
    /// In-memory cache of the last retrieved context (to avoid repeated DB hits)
    context_cache: RwLock<Option<ContextCache>>,
}

impl std::fmt::Debug for MemoryManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryManager")
            .field("store", &self.store)
            .field("config", &self.config)
            .field("pipeline", &self.pipeline.is_some())
            .field("vector_service", &self.vector_service.is_some())
            .field("session_search", &self.session_search.is_some())
            .field("context_cache", &self.context_cache)
            .finish()
    }
}

/// Cached context to avoid repeated DB hits within a short window.
#[derive(Debug, Clone)]
struct ContextCache {
    user_id: String,
    conversation_id: String,
    memories: Vec<Memory>,
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
    /// Create a new MemoryManager with the given store.
    pub fn new(store: Arc<UnifiedStore>, config: MemoryManagerConfig) -> Self {
        Self {
            store,
            config,
            pipeline: None,
            vector_service: None,
            session_search: None,
            context_cache: RwLock::new(None),
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

        let id = self.store.store(memory).await?;
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

        // ── Hybrid path ───────────────────────────────────────────────────────
        if let (Some(ref vs), Some(ref ss)) = (&self.vector_service, &self.session_search) {
            let mut cfg = self.config.hybrid_config.clone();
            cfg.max_results = limit;

            let hybrid_results = hybrid_search(&query_text, vs, ss, &cfg).await;

            let memories: Vec<Memory> = hybrid_results
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

            // Update cache
            if let Some(conv_id) = conversation_id {
                let cache = ContextCache {
                    user_id: user_id.to_string(),
                    conversation_id: conv_id.to_string(),
                    memories: memories.clone(),
                    cached_at: std::time::Instant::now(),
                };
                *self.context_cache.write().await = Some(cache);
            }

            return Ok(memories);
        }

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

        let memories = self.store.search(mq).await?;

        // Update cache
        if let Some(conv_id) = conversation_id {
            let cache = ContextCache {
                user_id: user_id.to_string(),
                conversation_id: conv_id.to_string(),
                memories: memories.clone(),
                cached_at: std::time::Instant::now(),
            };
            *self.context_cache.write().await = Some(cache);
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
                            .store
                            .get_conversation_history(conversation_id, 50)
                            .await?,
                        memories: cache.memories.clone(),
                    });
                }
            }
        }

        // Episodic: recent messages
        let messages = self
            .store
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

        Ok(SessionContext {
            messages,
            memories,
        })
    }

    /// Remember a user message (store in chat history).
    pub async fn remember_message(
        &self,
        user_id: impl Into<String>,
        conversation_id: impl Into<String>,
        role: impl Into<String>,
        content: impl Into<String>,
    ) -> crate::Result<()> {
        let msg =
            ChatMessage::new(conversation_id, user_id, role, content);
        self.store.store_message(msg).await
    }

    /// Get the last conversation ID for a user.
    pub async fn last_conversation(
        &self,
        user_id: impl AsRef<str>,
    ) -> crate::Result<Option<String>> {
        self.store.get_last_conversation(user_id.as_ref()).await
    }

    /// Forget (delete) a memory by ID.
    pub async fn forget(&self, id: &MemoryId) -> crate::Result<bool> {
        self.store.delete(id).await
    }

    /// Compact a session: extract key facts from old messages into semantic memories.
    ///
    /// This is called when a session is closed or exceeds thresholds
    /// (>50 turns or >7 days old).
    pub async fn compact_session(
        &self,
        conversation_id: impl AsRef<str>,
        _model: Option<&str>,
    ) -> crate::Result<Vec<MemoryId>> {
        let conversation_id = conversation_id.as_ref();
        info!("Compacting session: {}", conversation_id);

        // Get full session history
        let messages = self
            .store
            .get_conversation_history(conversation_id, 1000)
            .await?;

        if messages.len() < 10 {
            debug!("Session too short to compact: {} messages", messages.len());
            return Ok(vec![]);
        }

        // Simple extraction: just take every Nth user message as a key fact
        // In production, this would use an LLM to extract facts
        let mut stored_ids = vec![];
        let user_id = messages
            .iter()
            .find(|m| !m.user_id.is_empty())
            .map(|m| m.user_id.clone())
            .unwrap_or_else(|| "unknown".to_string());

        for (i, msg) in messages.iter().enumerate() {
            if msg.role != "user" {
                continue;
            }
            // Sample every 5th message
            if i % 5 != 0 {
                continue;
            }
            let fact = msg.content.clone();
            if fact.len() < 20 {
                continue;
            }

            let id = self
                .observe(
                    &user_id,
                    fact,
                    "semantic", // Memory type: extracted fact
                    0.6,        // Medium importance
                )
                .await?;

            stored_ids.push(id);
        }

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

        info!(
            "Session {} compacted: {} facts extracted",
            conversation_id,
            stored_ids.len()
        );
        Ok(stored_ids)
    }

    /// Get memory statistics.
    pub async fn stats(&self) -> crate::Result<MemoryStats> {
        self.store.stats().await
    }
}

/// Session context returned by `session_context()`.
#[derive(Debug, Clone)]
pub struct SessionContext {
    /// Recent chat messages (episodic memory)
    pub messages: Vec<ChatMessage>,
    /// Relevant semantic memories
    pub memories: Vec<Memory>,
}

impl SessionContext {
    /// Format the context as a system message injection.
    ///
    /// This produces the string that gets injected into the agent's
    /// context window before the conversation.
    pub fn format_for_injection(&self) -> String {
        let mut parts = vec![];

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

    pub async fn build(
        self,
        database_url: impl AsRef<str>,
    ) -> crate::Result<MemoryManager> {
        let store = Arc::new(UnifiedStore::new(database_url.as_ref()).await?);
        let mut mm = MemoryManager::new(store, self.config);

        if let Some(pipeline) = self.pipeline {
            mm = mm.with_pipeline(pipeline);
        }
        if let Some(vs) = self.vector_service {
            mm = mm.with_vector_service(vs);
        }
        if let Some(ss) = self.session_search {
            mm = mm.with_session_search(ss);
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
        let mm = MemoryManager::new(store, MemoryManagerConfig::default());

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
        let mm = MemoryManager::new(store, MemoryManagerConfig::default());

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
        let ctx = mm.session_context("user1", "conv1", Some::<&str>("food")).await.unwrap();

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
        };

        let formatted = ctx.format_for_injection();
        assert!(formatted.contains("Relevant Context"));
        assert!(formatted.contains("Likes coffee"));
    }
}
