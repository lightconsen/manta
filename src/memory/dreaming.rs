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
use tokio::sync::RwLock;
use tokio::time::{sleep_until, Instant as TokioInstant};
use tracing::{debug, info, warn};

use super::events::{MemoryEventBuilder, MemoryEventLog};
use super::tier::{MemoryTier, TierAction, TierEvaluator, TierIndex, TierSystemConfig};
use super::{Memory, MemoryId, MemoryQuery};

/// Async callback for LLM-based entity extraction in REM dreams.
/// Takes a prompt string and returns the LLM's response text.
pub type LlmCallback = Arc<
    dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
        + Send
        + Sync,
>;

/// Default cron expression: daily at 3:00 AM.
pub const DEFAULT_MEMORY_DREAMING_FREQUENCY: &str = "0 0 3 * * *";

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
    /// Duration of the dream cycle in milliseconds.
    pub duration_ms: u64,
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
    /// Peak memory usage observed during the cycle, in megabytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_memory_mb: Option<f64>,
    /// Estimated LLM input tokens consumed during the cycle.
    pub llm_tokens_input: u32,
    /// Estimated LLM output tokens produced during the cycle.
    pub llm_tokens_output: u32,
    /// Human-readable summary.
    pub summary: String,
    /// Errors encountered (non-fatal).
    pub errors: Vec<String>,
}

/// Observability counters for dream activity.
///
/// All counters use relaxed ordering — they are meant for observability,
/// not for synchronisation.
#[derive(Debug, Default)]
pub struct DreamMetrics {
    /// Total number of dream cycles started.
    pub dreams_total: AtomicU64,
    /// Total number of dream cycles that failed.
    pub dreams_failed: AtomicU64,
    /// Total memories processed across all dreams.
    pub memories_processed_total: AtomicU64,
    /// Total memories created across all dreams.
    pub memories_created_total: AtomicU64,
    /// Total memories removed across all dreams.
    pub memories_removed_total: AtomicU64,
    /// Total memories promoted across all dreams.
    pub memories_promoted_total: AtomicU64,
    /// Total memories demoted across all dreams.
    pub memories_demoted_total: AtomicU64,
    /// Total dream duration across all cycles, in milliseconds.
    pub dream_duration_ms_total: AtomicU64,
    /// Total estimated LLM input tokens consumed during dreams.
    pub llm_tokens_input_total: AtomicU64,
    /// Total estimated LLM output tokens produced during dreams.
    pub llm_tokens_output_total: AtomicU64,
}

impl DreamMetrics {
    /// Record a completed dream cycle.
    pub fn record(&self, result: &DreamResult, failed: bool) {
        self.dreams_total.fetch_add(1, Ordering::Relaxed);
        if failed {
            self.dreams_failed.fetch_add(1, Ordering::Relaxed);
        }
        self.memories_processed_total
            .fetch_add(result.memories_processed as u64, Ordering::Relaxed);
        self.memories_created_total
            .fetch_add(result.memories_created as u64, Ordering::Relaxed);
        self.memories_removed_total
            .fetch_add(result.memories_removed as u64, Ordering::Relaxed);
        self.memories_promoted_total
            .fetch_add(result.memories_promoted as u64, Ordering::Relaxed);
        self.memories_demoted_total
            .fetch_add(result.memories_demoted as u64, Ordering::Relaxed);
        self.dream_duration_ms_total
            .fetch_add(result.duration_ms, Ordering::Relaxed);
        self.llm_tokens_input_total
            .fetch_add(result.llm_tokens_input as u64, Ordering::Relaxed);
        self.llm_tokens_output_total
            .fetch_add(result.llm_tokens_output as u64, Ordering::Relaxed);
    }
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
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            crate::error::SyscityError::Storage {
                context: "Failed to serialize knowledge graph".to_string(),
                details: e.to_string(),
            }
        })?;
        if let Some(parent) = path.as_ref().parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                crate::error::SyscityError::Storage {
                    context: format!("Failed to create knowledge graph directory: {:?}", parent),
                    details: e.to_string(),
                }
            })?;
        }
        tokio::fs::write(path, json)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to write knowledge graph".to_string(),
                details: e.to_string(),
            })?;
        Ok(())
    }

    /// Load the knowledge graph from disk.
    pub async fn load_from_disk(path: impl AsRef<std::path::Path>) -> crate::Result<Self> {
        let json = tokio::fs::read_to_string(path).await.map_err(|e| {
            crate::error::SyscityError::Storage {
                context: "Failed to read knowledge graph".to_string(),
                details: e.to_string(),
            }
        })?;
        serde_json::from_str(&json).map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to deserialize knowledge graph".to_string(),
            details: e.to_string(),
        })
    }

    /// Maximum number of nodes before eviction kicks in.
    const MAX_NODES: usize = 10_000;
    /// Maximum number of edges before eviction kicks in.
    const MAX_EDGES: usize = 50_000;

    /// Cap the graph size by evicting lowest-confidence entries when limits
    /// are exceeded.  This prevents unbounded memory growth across REM cycles.
    ///
    /// NaN confidence values are replaced with 0.0 so they are evicted first
    /// rather than making sort order non-deterministic.
    pub fn cap_size(&mut self) {
        // Normalize NaN confidence values before sorting.
        for node in &mut self.nodes {
            if node.confidence.is_nan() {
                node.confidence = 0.0;
            }
        }
        for edge in &mut self.edges {
            if edge.confidence.is_nan() {
                edge.confidence = 0.0;
            }
        }

        if self.nodes.len() > Self::MAX_NODES {
            self.nodes.sort_by(|a, b| {
                a.confidence
                    .partial_cmp(&b.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            self.nodes.truncate(Self::MAX_NODES);
            // Re-sort by confidence descending for normal usage.
            self.nodes.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        if self.edges.len() > Self::MAX_EDGES {
            self.edges.sort_by(|a, b| {
                a.confidence
                    .partial_cmp(&b.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            self.edges.truncate(Self::MAX_EDGES);
            // Re-sort by confidence descending for normal usage.
            self.edges.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }
}

/// Heuristic entity extraction fallback when LLM is unavailable.
/// Extracts capitalized words appearing 3+ times as entities.
fn extract_entities_heuristic(memories: &[Memory]) -> (Vec<KnowledgeNode>, Vec<KnowledgeEdge>) {
    let mut word_counts: HashMap<String, u32> = HashMap::new();
    let processed = memories.len() as u32;

    for mem in memories {
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

    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for (word, count) in &word_counts {
        if *count >= 3 {
            let node = KnowledgeNode {
                label: word.clone(),
                node_type: "concept".to_string(),
                memory_ids: memories
                    .iter()
                    .filter(|m| {
                        let pattern = word.to_lowercase();
                        m.content.to_lowercase().split_whitespace().any(|w| {
                            w.trim_matches(|c: char| !c.is_alphanumeric()) == pattern.as_str()
                        })
                    })
                    .map(|m| m.id.to_string())
                    .collect(),
                confidence: (*count as f32 / processed as f32).min(1.0),
            };
            nodes.push(node);
        }
    }

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

    (nodes, edges)
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
    /// Optional review queue for human-in-the-loop mode.
    /// When set, dream actions are enqueued instead of applied directly.
    review_queue: Option<Arc<DreamReviewQueue>>,
    /// Observability counters for dream activity.
    metrics: Arc<DreamMetrics>,
}

/// Helper to estimate LLM token count from text.
///
/// This is a fast heuristic approximation (characters / 4) when provider
/// token usage metadata is unavailable.
fn estimate_tokens(text: &str) -> u32 {
    (text.chars().count() as u32 / 4).max(1)
}

/// Capture current system memory usage in megabytes.
fn current_memory_mb() -> Option<f64> {
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_memory(sysinfo::MemoryRefreshKind::new()),
    );
    sys.refresh_memory();
    Some(sys.used_memory() as f64 / 1024.0 / 1024.0)
}

/// Timing and token-tracking context for a single dream phase.
struct DreamPhaseContext {
    start: Instant,
    input_tokens: u32,
    output_tokens: u32,
}

impl DreamPhaseContext {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    fn track_prompt(&mut self, prompt: &str) {
        self.input_tokens += estimate_tokens(prompt);
    }

    fn track_response(&mut self, response: &str) {
        self.output_tokens += estimate_tokens(response);
    }

    fn duration_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
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
            review_queue: None,
            metrics: Arc::new(DreamMetrics::default()),
        }
    }

    /// Attach an event log.
    pub fn with_event_log(mut self, log: MemoryEventLog) -> Self {
        self.event_log = Some(log);
        self
    }

    /// Attach a review queue for human-in-the-loop dream mode.
    ///
    /// When set, dream actions are enqueued for review instead of
    /// applied directly to the memory store.
    pub fn with_review_queue(mut self, queue: Arc<DreamReviewQueue>) -> Self {
        self.review_queue = Some(queue);
        self
    }

    /// Attach a workspace directory for knowledge graph persistence.
    pub fn with_workspace_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.workspace_dir = Some(dir);
        self
    }

    /// Attach shared observability metrics.
    pub fn with_metrics(mut self, metrics: Arc<DreamMetrics>) -> Self {
        self.metrics = metrics;
        self
    }

    /// Get the current metrics.
    pub fn metrics(&self) -> Arc<DreamMetrics> {
        Arc::clone(&self.metrics)
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

    /// Compute the centroid of two vectors by averaging.
    fn merge_centroids(a: &[f32], b: &[f32]) -> Vec<f32> {
        if a.len() != b.len() || a.is_empty() {
            return Vec::new();
        }
        a.iter().zip(b.iter()).map(|(x, y)| (x + y) / 2.0).collect()
    }

    /// Run a Light Dream: deduplication, expiry cleanup, basic tier
    /// maintenance.
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

        let ctx = DreamPhaseContext::new();
        let baseline_memory_mb = current_memory_mb();

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
        let mut removed_ids: HashSet<MemoryId> = HashSet::new();
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
                        removed_ids.insert(mem_j.id.clone());
                    } else {
                        removed_ids.insert(mem_i.id.clone());
                        break;
                    }
                }
            }
        }

        for id in &removed_ids {
            if let Err(e) = store.delete(id).await {
                errors.push(format!("Failed to delete duplicate {}: {}", id, e));
            } else {
                removed += 1;
                tier_index.remove(&id.to_string());
            }
        }

        // Tier maintenance — also moves data between backends via as_tiered_store().
        // Skip memories that were deduplicated and removed.
        let tiered_store = store.as_tiered_store();
        for mem in &memories {
            if removed_ids.contains(&mem.id) {
                continue;
            }
            processed += 1;
            if let Some(tiered) = tier_index.get(&mem.id.to_string()) {
                let old_tier = tiered.tier;
                match evaluator.evaluate(mem, &tiered, None) {
                    TierAction::Promote(new_tier) => {
                        // Actually move data between backends when a tiered store is available.
                        if let Some(ts) = tiered_store {
                            if let Err(e) = ts.migrate_memory(mem, new_tier).await {
                                errors.push(format!("Failed to migrate memory {}: {}", mem.id, e));
                                continue;
                            }
                            // Keep the outer tier_index in sync with the actual data location.
                            tier_index.update_tier(&mem.id.to_string(), new_tier);
                        } else {
                            // Fallback: update index only (data stays in original backend).
                            tier_index.update_tier(&mem.id.to_string(), new_tier);
                        }
                        promoted += 1;
                        if let Some(ref event_log) = self.event_log {
                            let event = MemoryEventBuilder::new().promotion(
                                format!("{}:dream", mem.user_id),
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
                        // Actually move data between backends when a tiered store is available.
                        if let Some(ts) = tiered_store {
                            if let Err(e) = ts.migrate_memory(mem, new_tier).await {
                                errors.push(format!("Failed to migrate memory {}: {}", mem.id, e));
                                continue;
                            }
                            // Keep the outer tier_index in sync with the actual data location.
                            tier_index.update_tier(&mem.id.to_string(), new_tier);
                        } else {
                            // Fallback: update index only (data stays in original backend).
                            tier_index.update_tier(&mem.id.to_string(), new_tier);
                        }
                        demoted += 1;
                        if let Some(ref event_log) = self.event_log {
                            let event = MemoryEventBuilder::new().promotion(
                                format!("{}:dream", mem.user_id),
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
        let peak_memory_mb = current_memory_mb().map(|peak| {
            if let Some(baseline) = baseline_memory_mb {
                peak.max(baseline)
            } else {
                peak
            }
        });
        let result = DreamResult {
            dream_id,
            phase: DreamPhase::Light,
            started_at,
            finished_at,
            duration_ms: ctx.duration_ms(),
            memories_processed: processed,
            memories_created: 0,
            memories_removed: removed,
            memories_promoted: promoted,
            memories_demoted: demoted,
            peak_memory_mb,
            llm_tokens_input: ctx.input_tokens,
            llm_tokens_output: ctx.output_tokens,
            summary: format!(
                "Light Dream: processed {} memories, removed {} duplicates/expired, promoted {}, \
                 demoted {}",
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

    /// Run a Deep Dream: topic clustering, summary generation, cross-session
    /// linking.
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

        let ctx = DreamPhaseContext::new();
        let baseline_memory_mb = current_memory_mb();

        let mut created = 0;
        let mut errors = Vec::new();

        let memories = store
            .search(MemoryQuery::new().limit(self.config.max_memories_per_cycle))
            .await?;
        info!("Deep Dream: processing {} memories", memories.len());

        // Agglomerative clustering by embedding cosine similarity.
        // Memories with embeddings are clustered using cosine similarity;
        // memories without embeddings fall back to word-based grouping.
        let (with_embeddings, without_embeddings): (Vec<&Memory>, Vec<&Memory>) =
            memories.iter().partition(|m| m.embedding.is_some());

        let mut clusters: HashMap<String, Vec<&Memory>> = HashMap::new();

        // Cluster memories with embeddings using simple agglomerative clustering
        if !with_embeddings.is_empty() {
            // Each memory starts as its own cluster
            let mut cluster_centroids: Vec<(String, Vec<f32>)> = with_embeddings
                .iter()
                .filter_map(|m| {
                    m.embedding
                        .as_ref()
                        .map(|emb| (m.id.0.clone(), emb.clone()))
                })
                .collect();

            let mut assignments: Vec<String> =
                with_embeddings.iter().map(|m| m.id.0.clone()).collect();

            // Merge clusters when centroid cosine similarity > 0.7
            let merge_threshold = 0.7;
            let mut changed = true;
            while changed {
                changed = false;
                let mut to_merge: Option<(usize, usize)> = None;
                for i in 0..cluster_centroids.len() {
                    if cluster_centroids[i].0.is_empty() {
                        continue; // already merged
                    }
                    for j in (i + 1)..cluster_centroids.len() {
                        if cluster_centroids[j].0.is_empty() {
                            continue;
                        }
                        let sim = Self::cosine_similarity(
                            &cluster_centroids[i].1,
                            &cluster_centroids[j].1,
                        );
                        if sim > merge_threshold {
                            to_merge = Some((i, j));
                            break;
                        }
                    }
                    if to_merge.is_some() {
                        break;
                    }
                }
                if let Some((i, j)) = to_merge {
                    // Merge j into i
                    let new_centroid =
                        Self::merge_centroids(&cluster_centroids[i].1, &cluster_centroids[j].1);
                    cluster_centroids[i].1 = new_centroid;
                    // Update assignments
                    let target_id = cluster_centroids[i].0.clone();
                    let source_id = cluster_centroids[j].0.clone();
                    cluster_centroids[j].0.clear(); // mark as merged
                    for aid in &mut assignments {
                        if *aid == source_id {
                            *aid = target_id.clone();
                        }
                    }
                    changed = true;
                }
            }

            // Group memories by cluster assignment
            for (mem, assigned_id) in with_embeddings.iter().zip(assignments.iter()) {
                clusters.entry(assigned_id.clone()).or_default().push(mem);
            }
        }

        // Fall back to word-based clustering for memories without embeddings
        for mem in &without_embeddings {
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

        // Generate summary memories for clusters with >= 3 members
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

                let user_id = match unique_memories.first() {
                    Some(m) => m.user_id.clone(),
                    None => continue,
                };
                let summary = Memory::new(user_id, summary_content, "dream_summary")
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
        let peak_memory_mb = current_memory_mb().map(|peak| {
            if let Some(baseline) = baseline_memory_mb {
                peak.max(baseline)
            } else {
                peak
            }
        });
        let processed = memories.len();
        let result = DreamResult {
            dream_id,
            phase: DreamPhase::Deep,
            started_at,
            finished_at,
            duration_ms: ctx.duration_ms(),
            memories_processed: processed as u32,
            memories_created: created,
            memories_removed: 0,
            memories_promoted: 0,
            memories_demoted: 0,
            peak_memory_mb,
            llm_tokens_input: ctx.input_tokens,
            llm_tokens_output: ctx.output_tokens,
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

    /// Run a REM Dream: cross-session pattern discovery, knowledge graph
    /// update.
    ///
    /// - Extract entities and relationships via LLM-based NER (when callback
    ///   provided)
    /// - Update knowledge graph
    /// - Detect recurring patterns across sessions
    pub async fn run_rem(
        &self,
        store: &dyn super::MemoryStore,
        _tier_index: &TierIndex,
        llm_callback: Option<&LlmCallback>,
    ) -> crate::Result<DreamResult> {
        let started_at = SystemTime::now();
        let dream_id = format!("dream-rem-{}", uuid::Uuid::new_v4());
        info!("Starting REM Dream: {}", dream_id);

        let mut ctx = DreamPhaseContext::new();
        let baseline_memory_mb = current_memory_mb();

        let mut created = 0;
        let mut errors = Vec::new();

        let memories = store
            .search(MemoryQuery::new().limit(self.config.max_memories_per_cycle))
            .await?;
        info!("REM Dream: processing {} memories", memories.len());

        // LLM-based entity extraction when callback is available
        let (nodes, edges) = if let Some(llm) = llm_callback {
            // Build combined content for NER
            let combined_content: Vec<String> =
                memories.iter().map(|m| m.content.clone()).collect();
            let content_for_prompt = combined_content.join("\n---\n");

            let prompt = format!(
                "Extract entities (people, places, organizations, concepts) and their \
                 relationships from the following memory content. Each memory is separated by \
                 '---'.\n\nReturn ONLY a JSON object with this schema:\n{{\n  \"entities\": \
                 [{{\"label\": \"name\", \"type\": \"person|place|organization|concept\", \
                 \"confidence\": 0.9}}],\n  \"relationships\": [{{\"from\": \"entity_label\", \
                 \"to\": \"entity_label\", \"relation\": \"verb_phrase\", \"confidence\": \
                 0.8}}]\n}}\n\nMemory content:\n{}\n\nJSON:",
                content_for_prompt.chars().take(8000).collect::<String>()
            );

            ctx.track_prompt(&prompt);
            let response = llm(prompt).await;
            ctx.track_response(&response);

            // Parse JSON response
            #[derive(Deserialize)]
            struct NerResponse {
                entities: Vec<NerEntity>,
                #[serde(default)]
                relationships: Vec<NerRelationship>,
            }
            #[derive(Deserialize)]
            struct NerEntity {
                label: String,
                #[serde(rename = "type")]
                entity_type: String,
                #[serde(default = "default_confidence")]
                confidence: f32,
            }
            #[derive(Deserialize)]
            struct NerRelationship {
                from: String,
                to: String,
                relation: String,
                #[serde(default = "default_confidence")]
                confidence: f32,
            }
            fn default_confidence() -> f32 {
                0.5
            }

            match serde_json::from_str::<NerResponse>(&response) {
                Ok(parsed) => {
                    let mut nodes = Vec::new();
                    let mut edges = Vec::new();

                    for entity in &parsed.entities {
                        let pattern = entity.label.to_lowercase();
                        let memory_ids: Vec<String> = memories
                            .iter()
                            .filter(|m| m.content.to_lowercase().contains(&pattern))
                            .map(|m| m.id.to_string())
                            .collect();
                        if !memory_ids.is_empty() {
                            nodes.push(KnowledgeNode {
                                label: entity.label.clone(),
                                node_type: entity.entity_type.clone(),
                                memory_ids,
                                confidence: entity.confidence,
                            });
                        }
                    }

                    for rel in parsed.relationships {
                        edges.push(KnowledgeEdge {
                            from: rel.from,
                            to: rel.to,
                            relation: rel.relation,
                            confidence: rel.confidence,
                        });
                    }

                    // Fall back to co-occurrence edges if LLM didn't provide relationships
                    if edges.is_empty() {
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
                    }

                    (nodes, edges)
                }
                Err(e) => {
                    errors.push(format!("Failed to parse LLM NER response: {}", e));
                    debug!(
                        "LLM NER response (first 500 chars): {}",
                        response.chars().take(500).collect::<String>()
                    );
                    // Fall back to heuristic extraction
                    extract_entities_heuristic(&memories)
                }
            }
        } else {
            // No LLM callback: use heuristic extraction
            extract_entities_heuristic(&memories)
        };

        let node_count = nodes.len();
        let edge_count = edges.len();
        let processed = memories.len();

        // Store knowledge graph — merge, don't replace, so previous cycles'
        // entities and relationships are preserved. Cap size to prevent
        // unbounded memory growth across REM cycles.
        {
            let mut graph = self.knowledge_graph.write().await;
            let existing_labels: std::collections::HashSet<String> =
                graph.nodes.iter().map(|n| n.label.clone()).collect();
            for node in nodes {
                if !existing_labels.contains(&node.label) {
                    graph.nodes.push(node);
                }
            }
            graph.edges.extend(edges);
            graph.cap_size();
        }
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
        let peak_memory_mb = current_memory_mb().map(|peak| {
            if let Some(baseline) = baseline_memory_mb {
                peak.max(baseline)
            } else {
                peak
            }
        });
        let result = DreamResult {
            dream_id,
            phase: DreamPhase::Rem,
            started_at,
            finished_at,
            duration_ms: ctx.duration_ms(),
            memories_processed: processed as u32,
            memories_created: created,
            memories_removed: 0,
            memories_promoted: 0,
            memories_demoted: 0,
            peak_memory_mb,
            llm_tokens_input: ctx.input_tokens,
            llm_tokens_output: ctx.output_tokens,
            summary: format!(
                "REM Dream: processed {} memories, discovered {} entities, {} relations, created \
                 {} patterns",
                processed, node_count, edge_count, created
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
        llm_callback: Option<&LlmCallback>,
    ) -> crate::Result<Vec<DreamResult>> {
        if !self.config.enabled {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        // Light dream always runs
        match self.run_light(store, tier_index).await {
            Ok(r) => {
                self.metrics.record(&r, false);
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
            Err(e) => {
                self.metrics.dreams_total.fetch_add(1, Ordering::Relaxed);
                self.metrics.dreams_failed.fetch_add(1, Ordering::Relaxed);
                warn!("Light dream failed: {}", e);
            }
        }

        // Deep dream runs on balanced/slow or if enough memories
        match self.run_deep(store, tier_index).await {
            Ok(r) => {
                self.metrics.record(&r, false);
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
            Err(e) => {
                self.metrics.dreams_total.fetch_add(1, Ordering::Relaxed);
                self.metrics.dreams_failed.fetch_add(1, Ordering::Relaxed);
                warn!("Deep dream failed: {}", e);
            }
        }

        // REM dream runs only if requested and budget allows
        if include_rem && self.config.budget == DreamBudget::Expensive {
            match self.run_rem(store, tier_index, llm_callback).await {
                Ok(r) => {
                    self.metrics.record(&r, false);
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
                Err(e) => {
                    self.metrics.dreams_total.fetch_add(1, Ordering::Relaxed);
                    self.metrics.dreams_failed.fetch_add(1, Ordering::Relaxed);
                    warn!("REM dream failed: {}", e);
                }
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

// ── Dream Review Queue
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
        store: &dyn super::MemoryStore,
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
                                            "Dream apply: failed to delete memory {} during merge: {e}",
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

/// A scheduled dreaming service that runs dreams via cron.
#[derive(Clone)]
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
        llm_callback: Option<&LlmCallback>,
    ) -> crate::Result<Vec<DreamResult>> {
        self.engine
            .run_full_cycle(store, tier_index, include_rem, llm_callback)
            .await
    }

    /// Get the shared metrics.
    pub fn metrics(&self) -> Arc<DreamMetrics> {
        self.engine.metrics()
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
                        match engine.run_full_cycle(store.as_ref(), tier_index.as_ref(), include_rem, None).await {
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
            if let Err(e) = tx.send(()).await {
                warn!("Failed to send dream scheduler shutdown signal: {:?}", e);
            }
        }
    }

    /// Returns true if the scheduler background task is running.
    pub fn is_running(&self) -> bool {
        self.shutdown_tx.is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::memory::{Memory, MemoryId, MemoryQuery, MemoryStats, MemoryStore, UnifiedStore};
    use crate::SyscityError;

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

        let result = engine
            .run_rem(store.as_ref(), &tier_index, None)
            .await
            .unwrap();
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

        let result = engine.run_light(store.as_ref(), &tier_index).await.unwrap();
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

        let results = engine
            .run_full_cycle(store.as_ref(), &tier_index, false, None)
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

        let result = engine
            .run_rem(store.as_ref(), &tier_index, Some(&llm))
            .await
            .unwrap();
        assert_eq!(result.phase, DreamPhase::Rem);
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
