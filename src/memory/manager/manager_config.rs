//! MemoryManager configuration.

use super::*;

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
    /// Optional context-window-aware filtering of retrieved memories.
    /// When `None`, no token-budget filtering is applied.
    pub context_window: Option<ContextWindowConfig>,
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
            context_window: None,
        }
    }
}
