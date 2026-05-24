//! Dreaming Engine — Background Memory Consolidation
//!
//! Simulates human sleep memory consolidation through three phases:
//! - Light: deduplication, tag cleanup, expiry removal (fast, cheap)
//! - Deep: topic clustering, summary generation, cross-session linking (medium)
//! - REM: cross-session pattern discovery, knowledge graph update (expensive, rare)
//!
//! Triggered via cron scheduling (`DEFAULT_MEMORY_DREAMING_FREQUENCY`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::events::{MemoryEventBuilder, MemoryEventLog};
use super::tier::{MemoryTier, TierAction, TierEvaluator, TierIndex, TierSystemConfig};
use super::{Memory, MemoryQuery};
use chrono::Utc;
use cron::Schedule as CronSchedule;
use std::str::FromStr;
use std::time::Duration;
use tokio::time::{sleep_until, Instant as TokioInstant};

/// Default cron expression: daily at 3:00 AM.
pub const DEFAULT_MEMORY_DREAMING_FREQUENCY: &str = "0 3 * * *";

/// Dream execution speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamSpeed {
    Fast,
    Balanced,
    Slow,
}

/// Dream thinking depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamThinking {
    Low,
    Medium,
    High,
}

/// Dream budget level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamBudget {
    Cheap,
    Medium,
    Expensive,
}

/// Configuration for the dreaming engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamConfig {
    /// Whether dreaming is enabled.
    pub enabled: bool,
    /// Cron expression for scheduling.
    pub frequency: String,
    /// Execution speed.
    pub speed: DreamSpeed,
    /// Thinking depth.
    pub thinking: DreamThinking,
    /// Budget level.
    pub budget: DreamBudget,
    /// Similarity threshold for deduplication (0.0-1.0).
    pub dedup_similarity_threshold: f32,
    /// Minimum memories to trigger a dream.
    pub min_memories: usize,
    /// Maximum memories to process per dream cycle.
    pub max_memories_per_cycle: usize,
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            frequency: DEFAULT_MEMORY_DREAMING_FREQUENCY.to_string(),
            speed: DreamSpeed::Balanced,
            thinking: DreamThinking::Medium,
            budget: DreamBudget::Medium,
            dedup_similarity_threshold: 0.95,
            min_memories: 10,
            max_memories_per_cycle: 500,
        }
    }
}

/// Dream phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamPhase {
    Light,
    Deep,
    Rem,
}

impl std::fmt::Display for DreamPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DreamPhase::Light => write!(f, "light"),
            DreamPhase::Deep => write!(f, "deep"),
            DreamPhase::Rem => write!(f, "rem"),
        }
    }
}

/// Result of a single dream cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamResult {
    /// Unique dream run ID.
    pub dream_id: String,
    /// Phase that ran.
    pub phase: DreamPhase,
    /// When the dream started.
    pub started_at: SystemTime,
    /// When the dream finished.
    pub finished_at: SystemTime,
    /// Number of memories processed.
    pub memories_processed: u32,
    /// Number of memories created (summaries, merged, etc.).
    pub memories_created: u32,
    /// Number of memories deduplicated/removed.
    pub memories_removed: u32,
    /// Number of memories promoted.
    pub memories_promoted: u32,
    /// Number of memories demoted.
    pub memories_demoted: u32,
    /// Human-readable summary.
    pub summary: String,
    /// Errors encountered (non-fatal).
    pub errors: Vec<String>,
}

/// Recovery checkpoint for resuming interrupted dreams.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamCheckpoint {
    /// Last processed memory ID.
    pub last_memory_id: Option<String>,
    /// Phase that was interrupted.
    pub phase: Option<DreamPhase>,
    /// Timestamp of the checkpoint.
    pub timestamp: SystemTime,
}

impl Default for DreamCheckpoint {
    fn default() -> Self {
        Self {
            last_memory_id: None,
            phase: None,
            timestamp: SystemTime::UNIX_EPOCH,
        }
    }
}

/// Lightweight knowledge graph node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeNode {
    /// Entity label.
    pub label: String,
    /// Entity type (person, place, concept, etc.).
    pub node_type: String,
    /// Related memory IDs.
    pub memory_ids: Vec<String>,
    /// Confidence score.
    pub confidence: f32,
}

/// Lightweight knowledge graph edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEdge {
    /// Source node label.
    pub from: String,
    /// Target node label.
    pub to: String,
    /// Relationship type.
    pub relation: String,
    /// Confidence score.
    pub confidence: f32,
}

/// In-memory knowledge graph (REM phase output).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KnowledgeGraph {
    pub nodes: Vec<KnowledgeNode>,
    pub edges: Vec<KnowledgeEdge>,
}

impl KnowledgeGraph {
    /// Save the knowledge graph to disk as JSON.
    pub async fn save_to_disk(&self, path: impl AsRef<std::path::Path>) -> crate::Result<()> {
        let json =
            serde_json::to_string_pretty(self).map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to serialize knowledge graph".to_string(),
                details: e.to_string(),
            })?;
        if let Some(parent) = path.as_ref().parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                crate::error::MantaError::Storage {
                    context: format!("Failed to create knowledge graph directory: {:?}", parent),
                    details: e.to_string(),
                }
            })?;
        }
        tokio::fs::write(path, json)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to write knowledge graph".to_string(),
                details: e.to_string(),
            })?;
        Ok(())
    }

    /// Load the knowledge graph from disk.
    pub async fn load_from_disk(path: impl AsRef<std::path::Path>) -> crate::Result<Self> {
        let json = tokio::fs::read_to_string(path).await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: "Failed to read knowledge graph".to_string(),
                details: e.to_string(),
            }
        })?;
        serde_json::from_str(&json).map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to deserialize knowledge graph".to_string(),
            details: e.to_string(),
        })
    }
}

/// The dreaming engine orchestrates background memory consolidation.
pub struct DreamEngine {
    config: DreamConfig,
    tier_config: TierSystemConfig,
    /// Last checkpoint for recovery.
    checkpoint: RwLock<DreamCheckpoint>,
    /// Knowledge graph built during REM dreams.
    knowledge_graph: RwLock<KnowledgeGraph>,
    /// Event log for recording promotions and dream completions.
    event_log: Option<MemoryEventLog>,
    /// Workspace directory for knowledge graph persistence.
    workspace_dir: Option<std::path::PathBuf>,
}

impl DreamEngine {
    /// Create a new dream engine.
    pub fn new(config: DreamConfig, tier_config: TierSystemConfig) -> Self {
        Self {
            config,
            tier_config,
            checkpoint: RwLock::new(DreamCheckpoint::default()),
            knowledge_graph: RwLock::new(KnowledgeGraph::default()),
            event_log: None,
            workspace_dir: None,
        }
    }

    /// Attach an event log.
    pub fn with_event_log(mut self, log: MemoryEventLog) -> Self {
        self.event_log = Some(log);
        self
    }

    /// Attach a workspace directory for knowledge graph persistence.
    pub fn with_workspace_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.workspace_dir = Some(dir);
        self
    }

    /// Initialize the engine: load persisted knowledge graph if available.
    pub async fn initialize(&self) {
        if let Some(ref dir) = self.workspace_dir {
            let kg_path = dir.join("memory/.dreams/knowledge_graph.json");
            if kg_path.exists() {
                match KnowledgeGraph::load_from_disk(&kg_path).await {
                    Ok(kg) => {
                        *self.knowledge_graph.write().await = kg;
                        info!("Loaded persisted knowledge graph from {:?}", kg_path);
                    }
                    Err(e) => warn!("Failed to load knowledge graph: {}", e),
                }
            }
        }
    }

    /// Persist the current knowledge graph to disk.
    async fn save_knowledge_graph(&self) {
        if let Some(ref dir) = self.workspace_dir {
            let kg_path = dir.join("memory/.dreams/knowledge_graph.json");
            let graph = self.knowledge_graph.read().await.clone();
            if let Err(e) = graph.save_to_disk(&kg_path).await {
                warn!("Failed to save knowledge graph: {}", e);
            } else {
                debug!("Saved knowledge graph to {:?}", kg_path);
            }
        }
    }

    /// Compute cosine similarity between two embedding vectors.
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        dot / (norm_a * norm_b)
    }

    /// Run a Light Dream: deduplication, expiry cleanup, basic tier maintenance.
    ///
    /// - Remove expired memories
    /// - Deduplicate by embedding similarity > threshold
    /// - Promote/demote based on tier rules
    pub async fn run_light(
        &self,
        store: &dyn super::MemoryStore,
        tier_index: &TierIndex,
    ) -> crate::Result<DreamResult> {
        let started_at = SystemTime::now();
        let dream_id = format!("dream-light-{}", uuid::Uuid::new_v4());
        info!("Starting Light Dream: {}", dream_id);

        let mut removed = 0;
        let mut promoted = 0;
        let mut demoted = 0;
        let mut processed = 0;
        let mut errors = Vec::new();

        // Fetch all memories
        let memories = store
            .search(MemoryQuery::new().limit(self.config.max_memories_per_cycle))
            .await?;
        info!("Light Dream: processing {} memories", memories.len());

        let evaluator = TierEvaluator::new(self.tier_config.clone());

        // Deduplication: compare embedding cosine similarity; fall back to text hash
        let mut removed_ids = Vec::new();
        for (i, mem_i) in memories.iter().enumerate() {
            if removed_ids.contains(&mem_i.id) {
                continue;
            }
            for mem_j in memories.iter().skip(i + 1) {
                if removed_ids.contains(&mem_j.id) {
                    continue;
                }
                let similar = match (&mem_i.embedding, &mem_j.embedding) {
                    (Some(emb_i), Some(emb_j)) => {
                        Self::cosine_similarity(emb_i, emb_j)
                            > self.config.dedup_similarity_threshold
                    }
                    _ => {
                        // Fallback: first 50 chars text hash
                        let key_i = mem_i
                            .content
                            .to_lowercase()
                            .chars()
                            .take(50)
                            .collect::<String>();
                        let key_j = mem_j
                            .content
                            .to_lowercase()
                            .chars()
                            .take(50)
                            .collect::<String>();
                        key_i == key_j
                    }
                };
                if similar {
                    if mem_i.importance_score >= mem_j.importance_score {
                        removed_ids.push(mem_j.id.clone());
                    } else {
                        removed_ids.push(mem_i.id.clone());
                        break;
                    }
                }
            }
        }

        for id in removed_ids {
            if let Err(e) = store.delete(&id).await {
                errors.push(format!("Failed to delete duplicate {}: {}", id, e));
            } else {
                removed += 1;
                tier_index.remove(&id.to_string());
            }
        }

        // Tier maintenance
        for mem in &memories {
            processed += 1;
            if let Some(tiered) = tier_index.get(&mem.id.to_string()) {
                let old_tier = tiered.tier;
                match evaluator.evaluate(mem, &tiered) {
                    TierAction::Promote(new_tier) => {
                        tier_index.update_tier(&mem.id.to_string(), new_tier);
                        promoted += 1;
                        if let Some(ref event_log) = self.event_log {
                            let event = MemoryEventBuilder::new().promotion(
                                &format!("{}:dream", mem.user_id),
                                format!("promote-{}", uuid::Uuid::new_v4()),
                                old_tier.label(),
                                new_tier.label(),
                                "Dream tier evaluation",
                            );
                            if let Err(e) = event_log.append(&event).await {
                                warn!("Failed to append promotion event: {}", e);
                            }
                        }
                    }
                    TierAction::Demote(new_tier) => {
                        tier_index.update_tier(&mem.id.to_string(), new_tier);
                        demoted += 1;
                        if let Some(ref event_log) = self.event_log {
                            let event = MemoryEventBuilder::new().promotion(
                                &format!("{}:dream", mem.user_id),
                                format!("demote-{}", uuid::Uuid::new_v4()),
                                old_tier.label(),
                                new_tier.label(),
                                "Dream tier evaluation",
                            );
                            if let Err(e) = event_log.append(&event).await {
                                warn!("Failed to append promotion event: {}", e);
                            }
                        }
                    }
                    TierAction::Evict => {
                        if let Err(e) = store.delete(&mem.id).await {
                            errors.push(format!("Failed to evict {}: {}", mem.id, e));
                        } else {
                            removed += 1;
                            tier_index.remove(&mem.id.to_string());
                        }
                    }
                    TierAction::Keep => {}
                }
            }
        }

        let finished_at = SystemTime::now();
        let result = DreamResult {
            dream_id,
            phase: DreamPhase::Light,
            started_at,
            finished_at,
            memories_processed: processed,
            memories_created: 0,
            memories_removed: removed,
            memories_promoted: promoted,
            memories_demoted: demoted,
            summary: format!(
                "Light Dream: processed {} memories, removed {} duplicates/expired, promoted {}, demoted {}",
                processed, removed, promoted, demoted
            ),
            errors,
        };

        *self.checkpoint.write().await = DreamCheckpoint {
            last_memory_id: None,
            phase: Some(DreamPhase::Light),
            timestamp: finished_at,
        };

        info!("Light Dream complete: {}", result.summary);
        Ok(result)
    }

    /// Run a Deep Dream: topic clustering, summary generation, cross-session linking.
    ///
    /// - Cluster memories by embedding similarity
    /// - Generate summary memories for dense clusters
    /// - Link related memories across sessions
    pub async fn run_deep(
        &self,
        store: &dyn super::MemoryStore,
        tier_index: &TierIndex,
    ) -> crate::Result<DreamResult> {
        let started_at = SystemTime::now();
        let dream_id = format!("dream-deep-{}", uuid::Uuid::new_v4());
        info!("Starting Deep Dream: {}", dream_id);

        let mut created = 0;
        let mut processed = 0;
        let mut errors = Vec::new();

        let memories = store
            .search(MemoryQuery::new().limit(self.config.max_memories_per_cycle))
            .await?;
        info!("Deep Dream: processing {} memories", memories.len());

        // Simple clustering: group memories with shared words in content
        let mut clusters: HashMap<String, Vec<&Memory>> = HashMap::new();
        for mem in &memories {
            processed += 1;
            // Extract key words (naive: words > 4 chars)
            let words: Vec<String> = mem
                .content
                .split_whitespace()
                .filter(|w| w.len() > 4)
                .map(|w| {
                    w.to_lowercase()
                        .trim_matches(|c: char| !c.is_alphanumeric())
                        .to_string()
                })
                .filter(|w| !w.is_empty())
                .collect();

            for word in words.iter().take(3) {
                clusters.entry(word.clone()).or_default().push(mem);
            }
        }

        // Generate summary memories for clusters with > 2 members
        for (topic, cluster) in clusters {
            let mut unique_memories: Vec<&Memory> = cluster;
            unique_memories.sort_by_key(|m| &m.id.0);
            unique_memories.dedup_by(|a, b| a.id.0 == b.id.0);
            if unique_memories.len() >= 3 {
                let summaries: Vec<String> = unique_memories
                    .iter()
                    .map(|m| m.content.chars().take(80).collect::<String>())
                    .collect();

                let summary_content = format!("Topic '{}': {}", topic, summaries.join("; "));

                let summary = Memory::new(
                    unique_memories.iter().next().unwrap().user_id.clone(),
                    summary_content,
                    "dream_summary",
                )
                .with_importance_score(0.7)
                .with_source("dream_deep")
                .with_metadata(serde_json::json!({
                    "dream_phase": "deep",
                    "topic": topic,
                    "source_memory_count": unique_memories.len(),
                }));

                match store.store(summary).await {
                    Ok(id) => {
                        tier_index.insert(id.to_string(), MemoryTier::LongTerm);
                        created += 1;
                    }
                    Err(e) => {
                        errors.push(format!("Failed to store summary: {}", e));
                    }
                }
            }
        }

        let finished_at = SystemTime::now();
        let result = DreamResult {
            dream_id,
            phase: DreamPhase::Deep,
            started_at,
            finished_at,
            memories_processed: processed,
            memories_created: created,
            memories_removed: 0,
            memories_promoted: 0,
            memories_demoted: 0,
            summary: format!(
                "Deep Dream: processed {} memories, created {} topic summaries",
                processed, created
            ),
            errors,
        };

        *self.checkpoint.write().await = DreamCheckpoint {
            last_memory_id: None,
            phase: Some(DreamPhase::Deep),
            timestamp: finished_at,
        };

        info!("Deep Dream complete: {}", result.summary);
        Ok(result)
    }

    /// Run a REM Dream: cross-session pattern discovery, knowledge graph update.
    ///
    /// - Extract entities and relationships
    /// - Update knowledge graph
    /// - Detect recurring patterns across sessions
    pub async fn run_rem(
        &self,
        store: &dyn super::MemoryStore,
        _tier_index: &TierIndex,
    ) -> crate::Result<DreamResult> {
        let started_at = SystemTime::now();
        let dream_id = format!("dream-rem-{}", uuid::Uuid::new_v4());
        info!("Starting REM Dream: {}", dream_id);

        let mut created = 0;
        let mut processed = 0;
        let mut errors = Vec::new();

        let memories = store
            .search(MemoryQuery::new().limit(self.config.max_memories_per_cycle))
            .await?;
        info!("REM Dream: processing {} memories", memories.len());

        // Naive entity extraction: capitalize words that appear multiple times
        let mut word_counts: HashMap<String, u32> = HashMap::new();
        for mem in &memories {
            processed += 1;
            for word in mem.content.split_whitespace() {
                let clean = word
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .to_string();
                if clean.len() > 3
                    && clean
                        .chars()
                        .next()
                        .map(|c| c.is_uppercase())
                        .unwrap_or(false)
                {
                    *word_counts.entry(clean.clone()).or_insert(0) += 1;
                }
            }
        }

        // Build/update knowledge graph with frequent entities
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        for (word, count) in &word_counts {
            if *count >= 3 {
                let node = KnowledgeNode {
                    label: word.clone(),
                    node_type: "concept".to_string(),
                    memory_ids: memories
                        .iter()
                        .filter(|m| m.content.contains(word))
                        .map(|m| m.id.to_string())
                        .collect(),
                    confidence: (*count as f32 / processed as f32).min(1.0),
                };
                nodes.push(node);
            }
        }

        // Create edges between co-occurring entities
        for (i, n1) in nodes.iter().enumerate() {
            for n2 in nodes.iter().skip(i + 1) {
                let shared: Vec<_> = n1
                    .memory_ids
                    .iter()
                    .filter(|id| n2.memory_ids.contains(id))
                    .collect();
                if !shared.is_empty() {
                    edges.push(KnowledgeEdge {
                        from: n1.label.clone(),
                        to: n2.label.clone(),
                        relation: "co_occurs".to_string(),
                        confidence: (shared.len() as f32
                            / n1.memory_ids.len().max(n2.memory_ids.len()) as f32)
                            .min(1.0),
                    });
                }
            }
        }

        let node_count = nodes.len();
        let edge_count = edges.len();

        // Store knowledge graph
        let mut graph = self.knowledge_graph.write().await;
        graph.nodes = nodes;
        graph.edges = edges;
        drop(graph);
        self.save_knowledge_graph().await;

        // Create pattern memory from the graph
        if node_count > 0 {
            let pattern_mem = Memory::new(
                "system",
                format!(
                    "REM Dream discovered {} entities and {} relationships",
                    node_count, edge_count
                ),
                "dream_pattern",
            )
            .with_importance_score(0.8)
            .with_source("dream_rem")
            .with_metadata(serde_json::json!({
                "dream_phase": "rem",
                "entity_count": node_count,
                "relation_count": edge_count,
            }));

            match store.store(pattern_mem).await {
                Ok(_id) => {
                    created += 1;
                }
                Err(e) => {
                    errors.push(format!("Failed to store pattern memory: {}", e));
                }
            }
        }

        let finished_at = SystemTime::now();
        let result = DreamResult {
            dream_id,
            phase: DreamPhase::Rem,
            started_at,
            finished_at,
            memories_processed: processed,
            memories_created: created,
            memories_removed: 0,
            memories_promoted: 0,
            memories_demoted: 0,
            summary: format!(
                "REM Dream: processed {} memories, discovered {} entities, {} relations, created {} patterns",
                processed,
                node_count,
                edge_count,
                created
            ),
            errors,
        };

        *self.checkpoint.write().await = DreamCheckpoint {
            last_memory_id: None,
            phase: Some(DreamPhase::Rem),
            timestamp: finished_at,
        };

        info!("REM Dream complete: {}", result.summary);
        Ok(result)
    }

    /// Convert local DreamPhase to event-log DreamPhase.
    fn to_event_phase(phase: DreamPhase) -> super::events::DreamPhase {
        match phase {
            DreamPhase::Light => super::events::DreamPhase::Light,
            DreamPhase::Deep => super::events::DreamPhase::Deep,
            DreamPhase::Rem => super::events::DreamPhase::Rem,
        }
    }

    /// Run a full dream cycle: Light -> Deep -> (optional REM).
    pub async fn run_full_cycle(
        &self,
        store: &dyn super::MemoryStore,
        tier_index: &TierIndex,
        include_rem: bool,
    ) -> crate::Result<Vec<DreamResult>> {
        if !self.config.enabled {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        // Light dream always runs
        match self.run_light(store, tier_index).await {
            Ok(r) => {
                if let Some(ref event_log) = self.event_log {
                    let event = MemoryEventBuilder::new().dream(
                        &r.dream_id,
                        Self::to_event_phase(r.phase),
                        &r.summary,
                        r.memories_processed,
                        r.memories_created,
                    );
                    if let Err(e) = event_log.append(&event).await {
                        warn!("Failed to append dream event: {}", e);
                    }
                }
                results.push(r);
            }
            Err(e) => warn!("Light dream failed: {}", e),
        }

        // Deep dream runs on balanced/slow or if enough memories
        match self.run_deep(store, tier_index).await {
            Ok(r) => {
                if let Some(ref event_log) = self.event_log {
                    let event = MemoryEventBuilder::new().dream(
                        &r.dream_id,
                        Self::to_event_phase(r.phase),
                        &r.summary,
                        r.memories_processed,
                        r.memories_created,
                    );
                    if let Err(e) = event_log.append(&event).await {
                        warn!("Failed to append dream event: {}", e);
                    }
                }
                results.push(r);
            }
            Err(e) => warn!("Deep dream failed: {}", e),
        }

        // REM dream runs only if requested and budget allows
        if include_rem && self.config.budget == DreamBudget::Expensive {
            match self.run_rem(store, tier_index).await {
                Ok(r) => {
                    if let Some(ref event_log) = self.event_log {
                        let event = MemoryEventBuilder::new().dream(
                            &r.dream_id,
                            Self::to_event_phase(r.phase),
                            &r.summary,
                            r.memories_processed,
                            r.memories_created,
                        );
                        if let Err(e) = event_log.append(&event).await {
                            warn!("Failed to append dream event: {}", e);
                        }
                    }
                    results.push(r);
                }
                Err(e) => warn!("REM dream failed: {}", e),
            }
        }

        Ok(results)
    }

    /// Get the current knowledge graph.
    pub async fn knowledge_graph(&self) -> KnowledgeGraph {
        self.knowledge_graph.read().await.clone()
    }

    /// Get the last checkpoint.
    pub async fn checkpoint(&self) -> DreamCheckpoint {
        self.checkpoint.read().await.clone()
    }
}

/// A scheduled dreaming service that runs dreams via cron.
pub struct DreamScheduler {
    engine: Arc<DreamEngine>,
    /// Handle to the background scheduling task (for cancellation)
    shutdown_tx: Option<tokio::sync::mpsc::Sender<()>>,
}

impl DreamScheduler {
    /// Create a new scheduler around the given engine.
    pub fn new(engine: Arc<DreamEngine>) -> Self {
        Self { engine, shutdown_tx: None }
    }

    /// Run a one-off dream cycle immediately.
    pub async fn run_now(
        &self,
        store: &dyn super::MemoryStore,
        tier_index: &TierIndex,
        include_rem: bool,
    ) -> crate::Result<Vec<DreamResult>> {
        self.engine
            .run_full_cycle(store, tier_index, include_rem)
            .await
    }

    /// Get the engine configuration.
    pub fn config(&self) -> &DreamConfig {
        &self.engine.config
    }

    /// Start the background cron scheduler.
    ///
    /// Spawns a tokio task that sleeps until the next cron tick, runs the
    /// appropriate dream phase(s), then re-arms.  Call [`stop()`] to shut down.
    pub fn start(&mut self, store: Arc<dyn super::MemoryStore>, tier_index: Arc<TierIndex>) {
        if !self.engine.config.enabled {
            info!("Dreaming is disabled; scheduler not started");
            return;
        }

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel(1);
        self.shutdown_tx = Some(shutdown_tx);

        let engine = Arc::clone(&self.engine);
        let frequency = self.engine.config.frequency.clone();

        tokio::spawn(async move {
            let schedule = match CronSchedule::from_str(&frequency) {
                Ok(s) => s,
                Err(e) => {
                    warn!("Invalid dream cron expression '{}': {}", frequency, e);
                    return;
                }
            };

            loop {
                // Calculate next execution time
                let next = match schedule.upcoming(Utc).next() {
                    Some(dt) => dt,
                    None => {
                        warn!("No upcoming dream times for cron '{}'", frequency);
                        break;
                    }
                };

                let now = Utc::now();
                let delay_ms = if next > now {
                    (next - now).num_milliseconds().max(0) as u64
                } else {
                    0
                };

                let sleep_deadline = TokioInstant::now() + Duration::from_millis(delay_ms);
                info!("Next dream scheduled at {} (in {} ms)", next, delay_ms);

                tokio::select! {
                    _ = sleep_until(sleep_deadline) => {
                        info!("Running scheduled dream cycle");
                        let include_rem = engine.config.budget == DreamBudget::Expensive;
                        match engine.run_full_cycle(store.as_ref(), tier_index.as_ref(), include_rem).await {
                            Ok(results) => {
                                for r in &results {
                                    info!("Dream result: {}", r.summary);
                                }
                            }
                            Err(e) => {
                                warn!("Scheduled dream cycle failed: {}", e);
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Dream scheduler shutting down");
                        break;
                    }
                }
            }
        });
    }

    /// Stop the background scheduler.
    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
    }

    /// Returns true if the scheduler background task is running.
    pub fn is_running(&self) -> bool {
        self.shutdown_tx.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{DatabaseStore, MemoryStore, UnifiedStore};
    use std::sync::Arc;

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

        let result = engine.run_light(store.as_ref(), &tier_index).await.unwrap();
        assert_eq!(result.phase, DreamPhase::Light);
        assert!(result.memories_processed >= 5);
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

        let result = engine.run_deep(store.as_ref(), &tier_index).await.unwrap();
        assert_eq!(result.phase, DreamPhase::Deep);
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

        let result = engine.run_rem(store.as_ref(), &tier_index).await.unwrap();
        assert_eq!(result.phase, DreamPhase::Rem);
        assert!(result.memories_processed >= 5);

        let graph = engine.knowledge_graph().await;
        assert!(!graph.nodes.is_empty());
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
}
